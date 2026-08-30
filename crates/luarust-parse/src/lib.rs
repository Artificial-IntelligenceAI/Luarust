//! Luarust's grammar.
//!
//! Recursive descent, because the language is written to be read left to right and the
//! parser may as well be too. A statement announces itself with its first word, a chain
//! is a run of dotted words, and every list is bracketed — so nothing here needs to look
//! very far ahead.
//!
//! Parsing does not stop at the first mistake. When a statement cannot be understood the
//! parser steps to the end of it and carries on, so a file with four problems reports
//! four.

pub mod ast;

use ast::*;
use luarust_diag::{Diagnostic, Span};
use luarust_lex::{Kind, Token, literal_value, name_value, text_value};

/// A parsed file, and whatever could not be understood in it.
pub struct Parsed {
    pub program: Program,
    pub errors: Vec<Diagnostic>,
}

impl Parsed {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Read a token stream into a tree.
pub fn parse(source: &str, tokens: &[Token]) -> Parsed {
    let mut parser = Parser { source, tokens, at: 0, errors: Vec::new() };
    let program = parser.program();
    Parsed { program, errors: parser.errors }
}

/// Raised when a statement cannot be made sense of. Carries nothing: the diagnostic has
/// already been recorded, and this only says to stop and resynchronise.
struct Failed;

type Result<T> = std::result::Result<T, Failed>;

struct Parser<'a> {
    source: &'a str,
    tokens: &'a [Token],
    at: usize,
    errors: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    // ---- the token stream -------------------------------------------------------

    fn peek(&self) -> Token {
        self.tokens[self.at.min(self.tokens.len() - 1)]
    }

    fn peek_kind(&self) -> Kind {
        self.peek().kind
    }

    fn peek_at(&self, ahead: usize) -> Token {
        self.tokens[(self.at + ahead).min(self.tokens.len() - 1)]
    }

    fn text(&self, token: Token) -> &'a str {
        &self.source[token.span.start..token.span.end]
    }

    /// The word at the cursor, if there is one there.
    fn word(&self) -> Option<&'a str> {
        (self.peek_kind() == Kind::Word).then(|| self.text(self.peek()))
    }

    fn advance(&mut self) -> Token {
        let token = self.peek();
        if self.at + 1 < self.tokens.len() {
            self.at += 1;
        }
        token
    }

    fn eat(&mut self, kind: Kind) -> bool {
        if self.peek_kind() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: Kind, doing: &str) -> Result<Token> {
        if self.peek_kind() == kind {
            Ok(self.advance())
        } else {
            let found = self.peek();
            self.fail(
                Diagnostic::new("E0100", format!("{} was expected here.", kind.describe()))
                    .primary(found.span, format!("{} is here instead", found.kind.describe()))
                    .rule(doing.to_string())
                    .fix(format!("write {} here.", kind.describe())),
            )
        }
    }

    fn fail<T>(&mut self, diagnostic: Diagnostic) -> Result<T> {
        self.errors.push(diagnostic);
        Err(Failed)
    }

    /// Pick up after a statement that could not be understood, and be sure to have moved.
    ///
    /// [`Self::resynchronise`] stops at a `}` without eating it, so that a failure inside
    /// a loop body does not swallow the brace that ends the body. At the top level there
    /// is no body, nothing else consumes the brace, and the parser would otherwise try the
    /// same token again forever — reporting the same error until it ran out of memory.
    /// So: if recovering did not move, move.
    fn recover(&mut self) {
        let before = self.at;
        self.resynchronise();
        if self.at == before {
            self.advance();
            // And then get somewhere a statement could actually begin, rather than
            // reporting every bracket on the way out as a statement that is not one.
            self.resynchronise();
        }
    }

    /// Step forward to somewhere a new statement could plausibly begin, so that one
    /// misunderstood line does not turn into a hundred.
    fn resynchronise(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.peek_kind() {
                Kind::End => return,
                Kind::Semicolon if depth == 0 => {
                    self.advance();
                    return;
                }
                Kind::OpenBlock | Kind::OpenList | Kind::OpenGroup => {
                    depth += 1;
                    self.advance();
                }
                Kind::CloseBlock if depth == 0 => return,
                Kind::CloseBlock | Kind::CloseList | Kind::CloseGroup => {
                    depth = depth.saturating_sub(1);
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    // ---- statements -------------------------------------------------------------

    fn program(&mut self) -> Program {
        let mut stmts = Vec::new();
        while self.peek_kind() != Kind::End {
            match self.statement() {
                Ok(stmt) => stmts.push(stmt),
                Err(Failed) => self.recover(),
            }
        }
        Program { stmts }
    }

    fn statement(&mut self) -> Result<Stmt> {
        let start = self.peek();
        match self.word() {
            Some("var") => self.var_decl().map(Stmt::Var),
            Some("set") => self.set_stmt().map(Stmt::Set),
            Some("handback") => self.handback_stmt().map(Stmt::Handback),
            Some("print") => self.print_stmt().map(Stmt::Print),
            Some("loop") => self.loop_stmt(),
            Some("if") => self.if_stmt().map(Stmt::If),
            Some("fn") => self.func_decl().map(Stmt::Func),
            Some("return") => self.return_stmt().map(Stmt::Return),
            Some("break") => self.break_stmt().map(Stmt::Break),
            // Anything else that is a bare word before a list is a call written for its
            // effect. `print` is the one the language ships with; the rest are yours.
            Some(_) if self.peek_at(1).kind == Kind::OpenList => {
                let call = self.call()?;
                self.expect(Kind::Semicolon, "a statement ends with `;`")?;
                Ok(Stmt::Call(call))
            }
            Some("defaults") => self.defaults_stmt().map(Stmt::Defaults),
            Some(other) => self.fail(
                Diagnostic::new("E0101", format!("`{other}` does not begin a statement."))
                    .primary(start.span, "written here")
                    .rule("a statement begins with `var`, `set`, `handback`, `print`, `loop`, `if`, `fn`, `return`, `break` or `defaults`")
                    .tip("a name is written in quotes, so a bare word here is being read as a keyword.")
                    .fix("check the spelling, or put the statement's keyword in front of it."),
            ),
            None => self.fail(
                Diagnostic::new("E0101", "a statement was expected here.")
                    .primary(start.span, format!("{} is here instead", start.kind.describe()))
                    .rule("a statement begins with `var`, `set`, `handback`, `print`, `loop`, `if`, `fn`, `return`, `break` or `defaults`")
                    .fix("begin the statement with one of those words."),
            ),
        }
    }

    /// `var` `.chain…` `[` names `]` `=` `[` values `]` `;`
    fn var_decl(&mut self) -> Result<Var> {
        let start = self.advance().span; // `var`
        let hoisted = self.chain_words();

        let names_open = self.expect(Kind::OpenList, "a declaration names its variables in `[ ]`")?;
        let mut bindings = Vec::new();
        loop {
            bindings.push(self.binding(&hoisted)?);
            if !self.eat(Kind::Comma) {
                break;
            }
        }
        let names_close = self.expect(Kind::CloseList, "a list of names closes with `]`")?;

        self.expect(Kind::Equals, "a declaration gives its variables values with `=`")?;
        let (values, values_span) = self.value_list()?;
        let end = self.expect(Kind::Semicolon, "a statement that ends on a value ends with `;`")?;

        Ok(Var {
            span: start.to(end.span),
            bindings,
            values,
            names_span: names_open.span.to(names_close.span),
            values_span,
        })
    }

    /// One name in a declaration's list, with whatever it says about itself in front.
    fn binding(&mut self, hoisted: &[(String, Span)]) -> Result<Binding> {
        let start = self.peek().span;
        let mut own = Vec::new();
        while self.peek_kind() == Kind::Word {
            let token = self.advance();
            own.push((self.text(token).to_string(), token.span));
            if !self.eat(Kind::Dot) {
                break;
            }
        }

        let name_token = self.expect(Kind::Name, "every declared variable is named in quotes")?;
        let name = Ident {
            text: name_value(self.text(name_token)).to_string(),
            span: name_token.span,
        };

        let mut attrs = Attributes::default();
        for (word, span) in hoisted.iter().chain(own.iter()) {
            self.absorb(&mut attrs, word, *span);
        }

        let Some((ty, ty_span)) = attrs.ty else {
            return self.fail(
                Diagnostic::new("E0105", format!("`'{}'` is declared without a type.", name.text))
                    .primary(name.span, "no type was named for this")
                    .rule("every declaration states its type")
                    .tip("the type goes at the end of the chain, as in `var.local.b64`.")
                    .fix("add a type: `var.local.b64 ['{}'] = …;`".replace("{}", &name.text)),
            );
        };

        Ok(Binding {
            span: start.to(name.span),
            name,
            visibility: attrs.visibility.map_or(Visibility::Restricted, |(v, _)| v),
            visibility_span: attrs.visibility.map(|(_, span)| span),
            mutable: attrs.mutable.is_some(),
            mutable_span: attrs.mutable,
            ty,
            ty_span,
        })
    }

    /// Sort one chain word into the slot it belongs in, complaining if it fits none of
    /// them or if that slot is already taken.
    fn absorb(&mut self, attrs: &mut Attributes, word: &str, span: Span) {
        if let Some(visibility) = Visibility::from_word(word) {
            if let Some((existing, first)) = attrs.visibility {
                self.errors.push(
                    Diagnostic::new("E0103", format!("this says `{word}` after already saying `{}`.", existing.word()))
                        .secondary(first, "the visibility was settled here")
                        .primary(span, "and said again here")
                        .rule("a declaration names one visibility")
                        .fix("delete one of them."),
                );
                return;
            }
            attrs.visibility = Some((visibility, span));
        } else if word == "mut" {
            if let Some(first) = attrs.mutable {
                self.errors.push(
                    Diagnostic::new("E0103", "`mut` is said twice here.")
                        .secondary(first, "once here")
                        .primary(span, "and again here")
                        .rule("a declaration says `mut` at most once")
                        .fix("delete one of them."),
                );
                return;
            }
            attrs.mutable = Some(span);
        } else if let Some(ty) = Ty::from_word(word) {
            if let Some((existing, first)) = attrs.ty {
                self.errors.push(
                    Diagnostic::new("E0103", format!("this says `{word}` after already saying `{}`.", existing.word()))
                        .secondary(first, "the type was settled here")
                        .primary(span, "and said again here")
                        .rule("a declaration names one type")
                        .fix("delete one of them."),
                );
                return;
            }
            attrs.ty = Some((ty, span));
        } else {
            self.errors.push(
                Diagnostic::new("E0102", format!("`{word}` is not part of a declaration."))
                    .primary(span, "written here")
                    .rule("a declaration's chain holds a visibility, `mut`, and a type")
                    .tip("the visibilities are `local`, `global`, `public` and `restricted`.")
                    .fix("remove it, or replace it with a visibility, `mut`, or a type."),
            );
        }
    }

    /// `set` `[` names `]` `=` `[` values `]` `;`
    fn set_stmt(&mut self) -> Result<Set> {
        let start = self.advance().span; // `set`
        let open = self.expect(Kind::OpenList, "`set` names what it changes in `[ ]`")?;
        let mut targets = Vec::new();
        loop {
            let token = self.expect(Kind::Name, "`set` changes variables, which are named in quotes")?;
            targets.push(Ident { text: name_value(self.text(token)).to_string(), span: token.span });
            if !self.eat(Kind::Comma) {
                break;
            }
        }
        let close = self.expect(Kind::CloseList, "a list of names closes with `]`")?;
        self.expect(Kind::Equals, "`set` gives its variables values with `=`")?;
        let (values, values_span) = self.value_list()?;
        let end = self.expect(Kind::Semicolon, "a statement that ends on a value ends with `;`")?;
        Ok(Set {
            span: start.to(end.span),
            targets,
            values,
            names_span: open.span.to(close.span),
            values_span,
        })
    }

    /// `handback` `'source'` `as` `'target'` `;`
    fn handback_stmt(&mut self) -> Result<Handback> {
        let start = self.advance().span; // `handback`
        let source_token = self.expect(Kind::Name, "`handback` adds a variable, which is named in quotes")?;
        let source = Ident { text: name_value(self.text(source_token)).to_string(), span: source_token.span };

        match self.word() {
            Some("as") => {
                self.advance();
            }
            _ => {
                let found = self.peek();
                return self.fail(
                    Diagnostic::new("E0106", "`as` was expected here.")
                        .primary(found.span, format!("{} is here instead", found.kind.describe()))
                        .rule("`handback` is written `handback 'this' as 'that'`")
                        .fix("write `as` between the two names."),
                );
            }
        }

        let target_token = self.expect(Kind::Name, "`handback` adds into a variable, which is named in quotes")?;
        let target = Ident { text: name_value(self.text(target_token)).to_string(), span: target_token.span };
        let end = self.expect(Kind::Semicolon, "a statement that ends on a value ends with `;`")?;
        Ok(Handback { span: start.to(end.span), source, target })
    }

    /// `print` `[` items `]` `;`
    fn print_stmt(&mut self) -> Result<Print> {
        let start = self.advance().span; // `print`
        self.expect(Kind::OpenList, "`print` takes its items in `[ ]`")?;
        let mut items = Vec::new();
        while self.peek_kind() != Kind::CloseList {
            if self.peek_kind() == Kind::End {
                let at = self.peek().span;
                return self.fail(
                    Diagnostic::new("E0107", "this print list is never closed.")
                        .primary(at, "the file ends here")
                        .rule("`print` takes its items in `[ ]`")
                        .fix("add a `]` to close it."),
                );
            }
            items.push(self.print_item()?);
        }
        self.advance(); // `]`
        let end = self.expect(Kind::Semicolon, "a statement that ends on a value ends with `;`")?;
        Ok(Print { span: start.to(end.span), items })
    }

    fn print_item(&mut self) -> Result<PrintItem> {
        match self.peek_kind() {
            Kind::Text => {
                let token = self.advance();
                Ok(PrintItem::Text { value: text_value(self.text(token)), span: token.span })
            }
            Kind::Escape => {
                let token = self.advance();
                let raw = self.text(token);
                let value = match raw.chars().nth(1) {
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some('r') => '\r',
                    Some('0') => '\0',
                    _ => '\\',
                };
                Ok(PrintItem::Escape { value, span: token.span })
            }
            // A name in a print list is being read, not declared.
            Kind::Name => {
                let token = self.advance();
                Ok(PrintItem::Value(Expr::Name(Ident {
                    text: name_value(self.text(token)).to_string(),
                    span: token.span,
                })))
            }
            _ => self.value_expr().map(PrintItem::Value),
        }
    }

    /// `loop` `.temp|.perm` `.range` `.type` `[` name `]` `=` `[` from `,` to `]` `{` body `}`
    /// `loop.…` — a counting loop or a conditional one, told apart by the chain.
    fn loop_stmt(&mut self) -> Result<Stmt> {
        let start = self.advance().span; // `loop`
        let chain = self.chain_words();

        let mut lifetime: Option<(Lifetime, Span)> = None;
        let mut kind: Option<(String, Span)> = None;
        let mut ty: Option<(Ty, Span)> = None;
        for (word, span) in &chain {
            match word.as_str() {
                "temp" => lifetime = Some((Lifetime::Temp, *span)),
                "perm" => lifetime = Some((Lifetime::Perm, *span)),
                "range" | "while" => kind = Some((word.clone(), *span)),
                other => match Ty::from_word(other) {
                    Some(found) => ty = Some((found, *span)),
                    None => self.errors.push(
                        Diagnostic::new("E0102", format!("`{other}` is not part of a loop."))
                            .primary(*span, "written here")
                            .rule("a loop's chain says how long its counter lives, what kind of loop it is, and the counter's type")
                            .fix("use `temp` or `perm`, then `range` or `while`, then a type."),
                    ),
                },
            }
        }

        if kind.as_ref().is_some_and(|(word, _)| word == "while") {
            return self.while_loop(start, lifetime, ty).map(Stmt::While);
        }
        self.counting_loop(start, lifetime, kind, ty).map(Stmt::Loop)
    }

    /// `loop.while [ … ] { … }`, with a counter only if the chain asked for one.
    fn while_loop(
        &mut self,
        start: Span,
        lifetime: Option<(Lifetime, Span)>,
        ty: Option<(Ty, Span)>,
    ) -> Result<While> {
        let counter = match ty {
            None => {
                if let Some((_, span)) = lifetime {
                    self.errors.push(
                        Diagnostic::new("E0131", "this loop has no counter to say that about.".to_string())
                            .primary(span, "written here")
                            .rule("`temp` and `perm` say how long a counter lives, and a `while` loop only has one if it asks for one")
                            .tip("`loop.temp.while.ui32 ['n'] [ … ]` counts its passes; `loop.while [ … ]` does not.")
                            .fix("delete it, or add a type and a name."),
                    );
                }
                None
            }
            Some((ty, ty_span)) => {
                let Some((lifetime, lifetime_span)) = lifetime else {
                    return self.fail(
                        Diagnostic::new("E0108", "this loop does not say how long its counter lives.".to_string())
                            .primary(start, "written here")
                            .rule("a loop says `temp` or `perm`")
                            .tip("`temp` means the counter is gone at the closing brace; `perm` means it is still there afterwards, holding the last value it took.")
                            .fix("write `loop.temp.while…` or `loop.perm.while…`."),
                    );
                };
                self.expect(Kind::OpenList, "a loop names its counter in `[ ]`")?;
                let token = self.expect(Kind::Name, "a loop's counter is named in quotes")?;
                let name = Ident {
                    text: name_value(self.text(token)).to_string(),
                    span: token.span,
                };
                self.expect(Kind::CloseList, "a list of names closes with `]`")?;
                Some(Counter { name, ty, ty_span, lifetime, lifetime_span })
            }
        };

        self.expect(Kind::OpenList, "a condition is asked in `[ ]`")?;
        let condition = self.value_expr()?;
        self.expect(Kind::CloseList, "a condition closes with `]`")?;
        let (body, end) = self.braced_body("a loop")?;
        Ok(While { span: start.to(end), counter, condition, body })
    }

    /// `break;` or `break when reached <value>;`
    fn break_stmt(&mut self) -> Result<Break> {
        let start = self.advance().span; // `break`
        if self.eat(Kind::Semicolon) {
            return Ok(Break { span: start, reached: None });
        }

        let when = self.peek();
        if self.word() != Some("when") {
            return self.fail(
                Diagnostic::new("E0132", "a `break` either stops, or says when to.".to_string())
                    .primary(when.span, format!("{} is here instead", when.kind.describe()))
                    .rule("`break` is followed by `;` or by `when reached`")
                    .fix("write `break;`, or `break when reached <value>;`."),
            );
        }
        self.advance(); // `when`

        let reached = self.peek();
        if self.word() != Some("reached") {
            return self.fail(
                Diagnostic::new("E0132", "a `break when` says what was reached.".to_string())
                    .primary(reached.span, format!("{} is here instead", reached.kind.describe()))
                    .rule("`break` is followed by `;` or by `when reached`")
                    .fix("write `break when reached <value>;`."),
            );
        }
        self.advance(); // `reached`

        let value = self.value_expr()?;
        let end = self.expect(Kind::Semicolon, "a statement ends with `;`")?;
        Ok(Break { span: start.to(end.span), reached: Some(value) })
    }

    fn counting_loop(
        &mut self,
        start: Span,
        lifetime: Option<(Lifetime, Span)>,
        kind: Option<(String, Span)>,
        ty: Option<(Ty, Span)>,
    ) -> Result<Loop> {

        let Some((lifetime, lifetime_span)) = lifetime else {
            return self.fail(
                Diagnostic::new("E0108", "this loop does not say how long its counter lives.")
                    .primary(start, "written here")
                    .rule("a loop says `temp` or `perm`")
                    .tip("`temp` means the counter is gone at the closing brace; `perm` means it is still there afterwards, holding the last value it took.")
                    .fix("write `loop.temp.range…` or `loop.perm.range…`."),
            );
        };
        if kind.is_none() {
            return self.fail(
                Diagnostic::new("E0109", "this loop does not say what kind of loop it is.")
                    .primary(start, "written here")
                    .rule("a counting loop says `range` and a conditional one says `while`")
                    .fix(format!(
                        "write `loop.{0}.range…` to count, or `loop.{0}.while…` to keep going while something holds.",
                        if lifetime == Lifetime::Temp { "temp" } else { "perm" }
                    )),
            );
        }
        let Some((ty, ty_span)) = ty else {
            return self.fail(
                Diagnostic::new("E0105", "this loop does not say what type its counter is.")
                    .primary(start, "written here")
                    .rule("every declaration states its type, and a loop declares its counter")
                    .fix("add a type at the end of the chain, as in `loop.temp.range.ui8`."),
            );
        };

        self.expect(Kind::OpenList, "a loop names its counter in `[ ]`")?;
        let counter_token = self.expect(Kind::Name, "a loop's counter is named in quotes")?;
        let counter = Ident { text: name_value(self.text(counter_token)).to_string(), span: counter_token.span };
        self.expect(Kind::CloseList, "a list of names closes with `]`")?;
        self.expect(Kind::Equals, "a loop is given its bounds with `=`")?;

        let open = self.expect(Kind::OpenList, "a loop takes its bounds in `[ ]`")?;
        let from = self.value_expr()?;
        self.expect(Kind::Comma, "a range has two bounds, separated by `,`")?;
        let to = self.value_expr()?;
        let close = self.expect(Kind::CloseList, "a list of values closes with `]`")?;
        let _ = open.span.to(close.span);

        self.expect(Kind::OpenBlock, "a loop's body opens with `{`")?;
        let mut body = Vec::new();
        while self.peek_kind() != Kind::CloseBlock {
            if self.peek_kind() == Kind::End {
                return self.fail(
                    Diagnostic::new("E0107", "this loop's body is never closed.")
                        .primary(self.peek().span, "the file ends here")
                        .rule("a block opens with `{` and closes with `}`")
                        .fix("add a `}` to close it."),
                );
            }
            match self.statement() {
                Ok(stmt) => body.push(stmt),
                Err(Failed) => self.recover(),
            }
        }
        let end = self.advance(); // `}`

        Ok(Loop {
            span: start.to(end.span),
            lifetime,
            lifetime_span,
            counter,
            ty,
            ty_span,
            from,
            to,
            body,
        })
    }

    /// `fn.local.i32 ['bigger'] [i32 'a', i32 'b'] { … }`
    ///
    /// The chain says the visibility and what it answers. `nothing` there is the way to
    /// say it answers nothing -- absence is an error, because in this language a thing
    /// left unsaid never quietly means something.
    fn func_decl(&mut self) -> Result<Func> {
        let start = self.advance().span; // `fn`
        let chain = self.chain_words();

        let mut visibility: Option<(Visibility, Span)> = None;
        let mut returns: Option<(Option<Ty>, Span)> = None;
        for (word, span) in &chain {
            if let Some(found) = Visibility::from_word(word) {
                visibility = Some((found, *span));
            } else if word == "nothing" {
                returns = Some((None, *span));
            } else if let Some(found) = Ty::from_word(word) {
                returns = Some((Some(found), *span));
            } else {
                self.errors.push(
                    Diagnostic::new("E0120", format!("`{word}` is not part of a function."))
                        .primary(*span, "written here")
                        .rule("a function's chain says who can see it and what it answers")
                        .fix("use `local`, `global` or `public`, then a type or `nothing`."),
                );
            }
        }

        let Some((returns, returns_span)) = returns else {
            return self.fail(
                Diagnostic::new("E0121", "this function does not say what it answers.")
                    .primary(start, "written here")
                    .rule("a function states its answer, and says `nothing` when there is none")
                    .tip("a chain with no type in it is not the same as a function answering nothing -- one is left unsaid and the other is said.")
                    .fix("write a type, as in `fn.local.i32`, or `fn.local.nothing`."),
            );
        };

        self.expect(Kind::OpenList, "a function is named in `[ ]`")?;
        let name_token = self.expect(Kind::Name, "a function's name is written in quotes")?;
        let name = Ident {
            text: name_value(self.text(name_token)).to_string(),
            span: name_token.span,
        };
        self.expect(Kind::CloseList, "a list of names closes with `]`")?;

        // The parameters. An empty pair of brackets is a function that takes nothing.
        self.expect(Kind::OpenList, "a function takes its parameters in `[ ]`")?;
        let mut params = Vec::new();
        if self.peek_kind() != Kind::CloseList {
            loop {
                params.push(self.param()?);
                if !self.eat(Kind::Comma) {
                    break;
                }
            }
        }
        self.expect(Kind::CloseList, "a list of parameters closes with `]`")?;

        let (body, end) = self.braced_body("a function")?;

        Ok(Func {
            span: start.to(end),
            name,
            visibility: visibility.map_or(Visibility::Restricted, |(v, _)| v),
            visibility_span: visibility.map(|(_, span)| span),
            returns,
            returns_span,
            params,
            body,
        })
    }

    /// `name[a, b]` — a call, in a value slot or inside a math block alike.
    fn call(&mut self) -> Result<Expr> {
        let word = self.advance();
        let name = Ident { text: self.text(word).to_string(), span: word.span };
        self.expect(Kind::OpenList, "a call takes its arguments in `[ ]`")?;
        let mut args = Vec::new();
        if self.peek_kind() != Kind::CloseList {
            loop {
                args.push(self.value_expr()?);
                if !self.eat(Kind::Comma) {
                    break;
                }
            }
        }
        let close = self.expect(Kind::CloseList, "a list of arguments closes with `]`")?;
        Ok(Expr::Call { name, args, span: word.span.to(close.span) })
    }

    /// `i32 'a'` — the type, then the name it answers to inside the body.
    fn param(&mut self) -> Result<Param> {
        let start = self.peek();
        let Some(ty) = self.word().and_then(Ty::from_word) else {
            return self.fail(
                Diagnostic::new("E0122", "a parameter was expected here.")
                    .primary(start.span, format!("{} is here instead", start.kind.describe()))
                    .rule("a parameter states its type and then its name")
                    .fix("write a type and a name, as in `i32 'n'`."),
            );
        };
        let ty_span = self.advance().span;
        let name_token = self.expect(Kind::Name, "a parameter's name is written in quotes")?;
        Ok(Param {
            span: ty_span.to(name_token.span),
            ty,
            ty_span,
            name: Ident {
                text: name_value(self.text(name_token)).to_string(),
                span: name_token.span,
            },
        })
    }

    /// `return;` or `return <value>;`
    fn return_stmt(&mut self) -> Result<Return> {
        let start = self.advance().span; // `return`
        if self.peek_kind() == Kind::Semicolon {
            let end = self.advance().span;
            return Ok(Return { span: start.to(end), value: None });
        }
        let value = self.value_expr()?;
        let end = self.expect(Kind::Semicolon, "a statement ends with `;`")?;
        Ok(Return { span: start.to(end.span), value: Some(value) })
    }

    /// `if [ … ] { … } else-if [ … ] { … } else { … }`
    ///
    /// The `if` and its `else-if`s are the same shape and are kept in one list. `else`
    /// closes the chain, and nothing may follow it.
    fn if_stmt(&mut self) -> Result<If> {
        let start = self.advance().span; // `if`
        let mut arms = vec![self.arm(start)?];
        let mut otherwise = None;
        let mut end = arms[0].span;

        loop {
            match self.word() {
                Some("else-if") => {
                    if otherwise.is_some() {
                        let at = self.peek().span;
                        return self.fail(
                            Diagnostic::new("E0117", "this `else-if` comes after the `else`.")
                                .primary(at, "written here")
                                .rule("`else` is the last thing in an `if`, because nothing is left to ask")
                                .fix("move it above the `else`."),
                        );
                    }
                    let at = self.advance().span;
                    let arm = self.arm(at)?;
                    end = arm.span;
                    arms.push(arm);
                }
                Some("else") => {
                    if otherwise.is_some() {
                        let at = self.peek().span;
                        return self.fail(
                            Diagnostic::new("E0118", "this `if` has two `else`s.")
                                .primary(at, "written here")
                                .rule("an `if` has one `else`, which is what happens when nothing else did")
                                .fix("delete one of them, or make it an `else-if`."),
                        );
                    }
                    let at = self.advance().span;
                    let body = self.braced_body("an `else`")?;
                    end = at.to(body.1);
                    otherwise = Some(body.0);
                }
                _ => break,
            }
        }

        Ok(If { span: start.to(end), arms, otherwise })
    }

    /// The `[ condition ] { body }` that an `if` and an `else-if` both are.
    fn arm(&mut self, start: Span) -> Result<Arm> {
        self.expect(Kind::OpenList, "a condition is asked in `[ ]`")?;
        let condition = self.value_expr()?;
        self.expect(Kind::CloseList, "a condition closes with `]`")?;
        let (body, end) = self.braced_body("an `if`")?;
        Ok(Arm { span: start.to(end), condition, body })
    }

    /// `{ statements }`, and where it ended.
    fn braced_body(&mut self, whose: &str) -> Result<(Vec<Stmt>, Span)> {
        self.expect(Kind::OpenBlock, "a body opens with `{`")?;
        let mut body = Vec::new();
        while self.peek_kind() != Kind::CloseBlock {
            if self.peek_kind() == Kind::End {
                return self.fail(
                    Diagnostic::new("E0107", format!("{whose} body is never closed."))
                        .primary(self.peek().span, "the file ends here")
                        .rule("a block opens with `{` and closes with `}`")
                        .fix("add a `}` to close it."),
                );
            }
            match self.statement() {
                Ok(stmt) => body.push(stmt),
                Err(Failed) => self.recover(),
            }
        }
        let end = self.advance().span; // `}`
        Ok((body, end))
    }

    /// `defaults.setting.behaviour;`
    fn defaults_stmt(&mut self) -> Result<Defaults> {
        let start = self.advance().span; // `defaults`
        let chain = self.chain_words();
        if chain.len() != 2 {
            return self.fail(
                Diagnostic::new("E0110", "a default is a setting and a behaviour.")
                    .primary(start, "written here")
                    .rule("a default is written `defaults.setting.behaviour;`")
                    .fix("write something like `defaults.no-visibility-stated.error;`."),
            );
        }
        let end = self.expect(Kind::Semicolon, "a statement that ends on a value ends with `;`")?;
        let (setting, setting_span) = chain[0].clone();
        let (behaviour, behaviour_span) = chain[1].clone();
        Ok(Defaults { span: start.to(end.span), setting, setting_span, behaviour, behaviour_span })
    }

    /// The `.word.word` run after a keyword.
    fn chain_words(&mut self) -> Vec<(String, Span)> {
        let mut words = Vec::new();
        while self.peek_kind() == Kind::Dot && self.peek_at(1).kind == Kind::Word {
            self.advance(); // `.`
            let token = self.advance();
            words.push((self.text(token).to_string(), token.span));
        }
        words
    }

    // ---- values -----------------------------------------------------------------

    fn value_list(&mut self) -> Result<(Vec<Expr>, Span)> {
        let open = self.expect(Kind::OpenList, "values are given in `[ ]`")?;
        let mut values = Vec::new();
        loop {
            values.push(self.value_expr()?);
            if !self.eat(Kind::Comma) {
                break;
            }
        }
        let close = self.expect(Kind::CloseList, "a list of values closes with `]`")?;
        Ok((values, open.span.to(close.span)))
    }

    /// Something standing where a value stands: a literal, a math block, or the clock.
    fn value_expr(&mut self) -> Result<Expr> {
        match self.peek_kind() {
            // Bars hold a written value, read as whatever type is being asked for.
            Kind::Literal => {
                let token = self.advance();
                Ok(Expr::Literal {
                    text: literal_value(self.text(token)).to_string(),
                    span: token.span,
                })
            }
            // Quotes hold a name, here as everywhere else, so this reads the variable.
            Kind::Name => {
                let token = self.advance();
                Ok(Expr::Name(Ident {
                    text: name_value(self.text(token)).to_string(),
                    span: token.span,
                }))
            }
            Kind::Word if self.word() == Some("math") => {
                let start = self.advance().span;
                self.expect(Kind::OpenBlock, "`math` takes its arithmetic in `{ }`")?;
                let inner = self.condition()?;
                let close = self.expect(Kind::CloseBlock, "a math block closes with `}`")?;
                Ok(Expr::Math { inner: Box::new(inner), span: start.to(close.span) })
            }
            Kind::Word if self.word() == Some("time") => self.time_now(),
            // A bare word before a list is a call. A variable is always quoted, so
            // there is nothing else a bare word in front of `[` could be.
            Kind::Word if self.peek_at(1).kind == Kind::OpenList => self.call(),
            _ => {
                let found = self.peek();
                self.fail(
                    Diagnostic::new("E0111", "a value was expected here.")
                        .primary(found.span, format!("{} is here instead", found.kind.describe()))
                        .rule("a value is a written value in bars, a name in quotes, a `math { }` block, or `time.now`")
                        .tip("arithmetic only happens inside `math { }`, so `1 + 2` on its own is not a value.")
                        .fix("write `|…|`, a name in quotes, or wrap the arithmetic in `math { … }`."),
                )
            }
        }
    }

    fn time_now(&mut self) -> Result<Expr> {
        let start = self.advance().span; // `time`
        self.expect(Kind::Dot, "the clock is written `time.now`")?;
        match self.word() {
            Some("now") => {
                let end = self.advance().span;
                Ok(Expr::TimeNow { span: start.to(end) })
            }
            _ => {
                let found = self.peek();
                self.fail(
                    Diagnostic::new("E0112", "`now` was expected after `time`.")
                        .primary(found.span, "written here")
                        .rule("the clock is written `time.now`")
                        .fix("write `time.now`."),
                )
            }
        }
    }

    // ---- arithmetic -------------------------------------------------------------
    //
    // Loosest first. Exponent binds tightest and groups rightward, so `2 ** 3 ** 2` is
    // 512, and unary minus sits below it, so `-'x' ** 2` is `-('x' ** 2)` -- which is how
    // a mathematician reads it and, as it happens, how most languages do too.

    /// The loosest thing a math block holds: at most one comparison.
    ///
    /// Deliberately not a chain. Mathematics reads `a < b < c` as both comparisons at
    /// once, and most languages read it as comparing a `bool` to a number, which is
    /// nonsense — so rather than pick the wrong one quietly, a second comparison is an
    /// error that says the first one is what needs deciding.
    /// The whole of what a `math` block may hold, loosest first: `or`, then `and`, then
    /// `not`, then a comparison, then the arithmetic.
    ///
    /// `not` binds tighter than `and` and looser than a comparison, so `not 'a' = 'b'`
    /// asks whether they are *not* equal, which is how it reads aloud.
    fn condition(&mut self) -> Result<Expr> {
        let mut lhs = self.conjunction()?;
        while self.word() == Some("or") {
            self.advance();
            let rhs = self.conjunction()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Logic { op: LogicOp::Or, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn conjunction(&mut self) -> Result<Expr> {
        let mut lhs = self.negation()?;
        while self.word() == Some("and") {
            self.advance();
            let rhs = self.negation()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Logic { op: LogicOp::And, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn negation(&mut self) -> Result<Expr> {
        // A `not` with something after it turns that around. A `not` with an `=` after it
        // is the other `not`, the one that spells `!=`, and that is the comparison's to
        // read -- but it can only appear after something, so it is never seen here.
        if self.word() == Some("not") && self.peek_at(1).kind != Kind::Equals {
            let start = self.advance().span;
            let operand = self.negation()?;
            let span = start.to(operand.span());
            return Ok(Expr::Not { operand: Box::new(operand), span });
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Expr> {
        let lhs = self.sum()?;
        let op = match (self.peek_kind(), self.word()) {
            (Kind::Less, _) => CmpOp::Less,
            (Kind::Greater, _) => CmpOp::Greater,
            (Kind::Equals, _) => CmpOp::Equal,
            (Kind::LessEqual, _) => CmpOp::LessEqual,
            (Kind::GreaterEqual, _) => CmpOp::GreaterEqual,
            (Kind::NotEqual, _) => CmpOp::NotEqual,
            // `not=` is the word and the sign, which is a third way of writing `!=`.
            (Kind::Word, Some("not")) if self.peek_at(1).kind == Kind::Equals => {
                self.advance();
                CmpOp::NotEqual
            }
            _ => return Ok(lhs),
        };
        self.advance();
        let rhs = self.sum()?;
        let span = lhs.span().to(rhs.span());

        if matches!(
            self.peek_kind(),
            Kind::Less | Kind::Greater | Kind::Equals | Kind::LessEqual | Kind::GreaterEqual | Kind::NotEqual
        ) {
            let second = self.peek();
            return self.fail(
                Diagnostic::new("E0114", "there are two comparisons here.")
                    .secondary(span, "this one")
                    .primary(second.span, "and this one")
                    .rule("a comparison compares two things, and is not chained")
                    .tip("mathematics reads `a < b < c` as both at once; most languages read it as comparing a `bool` to a number, which is nothing. Luarust refuses rather than pick.")
                    .fix("compare two things at a time."),
            );
        }

        Ok(Expr::Compare { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span })
    }

    fn sum(&mut self) -> Result<Expr> {
        let mut lhs = self.product()?;
        loop {
            let op = match self.peek_kind() {
                Kind::Plus => BinOp::Add,
                Kind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let rhs = self.product()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn product(&mut self) -> Result<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let op = match (self.peek_kind(), self.word()) {
                (Kind::Star, _) => BinOp::Mul,
                (Kind::Slash, _) => BinOp::Div,
                (Kind::Word, Some("x")) => BinOp::Mul,
                (Kind::Word, Some("div")) => BinOp::Div,
                (Kind::Word, Some("mod")) => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let rhs = self.unary()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.peek_kind() == Kind::Minus {
            let start = self.advance().span;
            let operand = self.unary()?;
            let span = start.to(operand.span());
            return Ok(Expr::Unary { op: BinOp::Sub, operand: Box::new(operand), span });
        }
        self.power()
    }

    fn power(&mut self) -> Result<Expr> {
        let base = self.postfix()?;
        let is_power = matches!(
            (self.peek_kind(), self.word()),
            (Kind::StarStar, _) | (Kind::Word, Some("xx")) | (Kind::Word, Some("pow"))
        );
        if !is_power {
            return Ok(base);
        }
        self.advance();
        // Rightward, so `2 ** 3 ** 2` is 2 ** (3 ** 2).
        let exponent = self.unary()?;
        let span = base.span().to(exponent.span());
        Ok(Expr::Binary {
            op: BinOp::Pow,
            lhs: Box::new(base),
            rhs: Box::new(exponent),
            span,
        })
    }

    fn postfix(&mut self) -> Result<Expr> {
        let inner = self.primary()?;
        if self.peek_kind() == Kind::Percent {
            let end = self.advance().span;
            let span = inner.span().to(end);
            return Ok(Expr::Percent { inner: Box::new(inner), span });
        }
        Ok(inner)
    }

    fn primary(&mut self) -> Result<Expr> {
        match self.peek_kind() {
            Kind::Number => {
                let token = self.advance();
                Ok(Expr::Number { text: self.text(token).to_string(), span: token.span })
            }
            // A type word in front of a quoted value makes it a literal that says what it
            // is. Type names and the word-spelled operators share nothing, so `x` is
            // always multiplication and `ui32` is always a type.
            Kind::Word
                if self
                    .word()
                    .and_then(Ty::from_word)
                    .is_some_and(|_| self.peek_at(1).kind == Kind::Literal) =>
            {
                let ty_token = self.advance();
                let ty = Ty::from_word(self.text(ty_token)).expect("just checked");
                let value = self.advance();
                Ok(Expr::TypedLiteral {
                    ty,
                    text: literal_value(self.text(value)).to_string(),
                    span: ty_token.span.to(value.span),
                })
            }

            // A written value with no type in front of it, taking its type from whatever
            // is expecting it -- exactly as a bare number does.
            Kind::Literal => {
                let token = self.advance();
                Ok(Expr::Literal {
                    text: literal_value(self.text(token)).to_string(),
                    span: token.span,
                })
            }

            // Inside a math block the quotes hold a variable, not a literal.
            Kind::Name => {
                let token = self.advance();
                Ok(Expr::Name(Ident {
                    text: name_value(self.text(token)).to_string(),
                    span: token.span,
                }))
            }
            Kind::OpenGroup => {
                self.advance();
                let inner = self.condition()?;
                self.expect(Kind::CloseGroup, "a group opened with `(` closes with `)`")?;
                Ok(inner)
            }
            Kind::Word if self.word() == Some("time") => self.time_now(),
            // A bare word before a list is a call. A variable is always quoted, so
            // there is nothing else a bare word in front of `[` could be.
            Kind::Word if self.peek_at(1).kind == Kind::OpenList => self.call(),
            _ => {
                let found = self.peek();
                self.fail(
                    Diagnostic::new("E0113", "something to calculate with was expected here.")
                        .primary(found.span, format!("{} is here instead", found.kind.describe()))
                        .rule("a math block calculates with numbers, written values in bars, variables in quotes, and groups in `( )`")
                        .fix("write a number, a `|…|`, a variable in quotes, or a group in `( )`."),
                )
            }
        }
    }
}

/// The parts of a declaration's chain, as they are found.
#[derive(Default)]
struct Attributes {
    visibility: Option<(Visibility, Span)>,
    mutable: Option<Span>,
    ty: Option<(Ty, Span)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(source: &str) -> Parsed {
        let lexed = luarust_lex::lex(source);
        assert!(lexed.ok(), "lexing failed: {:#?}", lexed.errors);
        parse(source, &lexed.tokens)
    }

    fn clean(source: &str) -> Program {
        let out = parse_str(source);
        assert!(out.ok(), "expected no errors, got {:#?}", out.errors);
        out.program
    }

    fn codes(source: &str) -> Vec<String> {
        parse_str(source).errors.into_iter().map(|e| e.code).collect()
    }

    /// An expression as a nested list, so precedence is visible at a glance.
    fn show(expr: &Expr) -> String {
        match expr {
            Expr::Literal { text, .. } => format!("|{text}|"),
            Expr::Name(name) => name.text.clone(),
            Expr::Number { text, .. } => text.clone(),
            Expr::TimeNow { .. } => "time.now".to_string(),
            Expr::Percent { inner, .. } => format!("(% {})", show(inner)),
            Expr::Unary { operand, .. } => format!("(neg {})", show(operand)),
            Expr::Binary { op, lhs, rhs, .. } => {
                format!("({} {} {})", op.word(), show(lhs), show(rhs))
            }
            Expr::Math { inner, .. } => show(inner),
            Expr::TypedLiteral { ty, text, .. } => format!("{}|{text}|", ty.word()),
            Expr::Compare { op, lhs, rhs, .. } => {
                format!("({} {} {})", op.word(), show(lhs), show(rhs))
            }
            Expr::Logic { op, lhs, rhs, .. } => {
                format!("({} {} {})", op.word(), show(lhs), show(rhs))
            }
            Expr::Not { operand, .. } => format!("(not {})", show(operand)),
            Expr::Call { name, args, .. } => {
                let shown: Vec<String> = args.iter().map(show).collect();
                format!("({} {})", name.text, shown.join(" "))
            }
        }
    }

    /// Parse one math block and show its shape.
    fn math(expression: &str) -> String {
        let source = format!("var.local.b64 ['z'] = [math {{ {expression} }}];");
        let program = clean(&source);
        match &program.stmts[0] {
            Stmt::Var(var) => show(&var.values[0]),
            other => panic!("expected a declaration, got {other:?}"),
        }
    }

    #[test]
    fn the_readme_counting_program_parses() {
        let program = clean(
            "loop.temp.range.ui8 ['i'] = [|1|, |5|] {\n    print['i' \\n];\n}\n",
        );
        assert_eq!(program.stmts.len(), 1);
        let Stmt::Loop(loop_stmt) = &program.stmts[0] else { panic!("not a loop") };
        assert_eq!(loop_stmt.lifetime, Lifetime::Temp);
        assert_eq!(loop_stmt.ty, Ty::U8);
        assert_eq!(loop_stmt.counter.text, "i");
        assert_eq!(loop_stmt.body.len(), 1);
        assert!(matches!(loop_stmt.body[0], Stmt::Print(_)));
    }

    #[test]
    fn the_readme_accumulating_program_parses() {
        let program = clean(
            "var.local.mut.ui32 ['total'] = [|0|];\n\
             loop.temp.range.ui8 ['i'] = [|1|, |10|] {\n\
                 handback 'i' as 'total';\n\
             }\n\
             print[\"total is \" 'total' \\n];\n",
        );
        assert_eq!(program.stmts.len(), 3);

        let Stmt::Var(var) = &program.stmts[0] else { panic!("not a declaration") };
        assert_eq!(var.bindings.len(), 1);
        assert_eq!(var.bindings[0].name.text, "total");
        assert_eq!(var.bindings[0].visibility, Visibility::Local);
        assert!(var.bindings[0].mutable);
        assert_eq!(var.bindings[0].ty, Ty::U32);

        let Stmt::Loop(loop_stmt) = &program.stmts[1] else { panic!("not a loop") };
        let Stmt::Handback(handback) = &loop_stmt.body[0] else { panic!("not a handback") };
        assert_eq!((handback.source.text.as_str(), handback.target.text.as_str()), ("i", "total"));

        let Stmt::Print(print) = &program.stmts[2] else { panic!("not a print") };
        assert_eq!(print.items.len(), 3);
        assert!(matches!(&print.items[0], PrintItem::Text { value, .. } if value == "total is "));
        assert!(matches!(&print.items[1], PrintItem::Value(Expr::Name(n)) if n.text == "total"));
        assert!(matches!(print.items[2], PrintItem::Escape { value: '\n', .. }));
    }

    #[test]
    fn the_readme_timing_program_parses() {
        let program = clean(
            "var.local.mut.ui64 ['sum'] = [|0|];\n\
             var.local.b64 ['start']    = [time.now];\n\
             loop.temp.range.ui64 ['i'] = [|1|, |100000000|] {\n\
                 set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];\n\
             }\n\
             var.local.b64 ['elapsed'] = [math { time.now - 'start' }];\n\
             print['sum' \" in \" 'elapsed' \" seconds\\n\"];\n",
        );
        assert_eq!(program.stmts.len(), 5);
        let Stmt::Var(start) = &program.stmts[1] else { panic!("not a declaration") };
        assert!(matches!(start.values[0], Expr::TimeNow { .. }));
        let Stmt::Var(elapsed) = &program.stmts[3] else { panic!("not a declaration") };
        assert_eq!(show(&elapsed.values[0]), "(- time.now start)");
    }

    #[test]
    fn all_three_declaration_forms_say_the_same_thing() {
        // Everything hoisted onto `var`.
        let a = clean("var.local.mut.b16 ['a', 'b'] = [|1|, |2|];");
        // Scope hoisted, types inline.
        let b = clean("var.local.mut [b16 'a', b16 'b'] = [|1|, |2|];");
        // Nothing hoisted at all.
        let c = clean("var [local.mut.b16 'a', local.mut.b16 'b'] = [|1|, |2|];");

        for program in [&a, &b, &c] {
            let Stmt::Var(var) = &program.stmts[0] else { panic!("not a declaration") };
            assert_eq!(var.bindings.len(), 2);
            for binding in &var.bindings {
                assert_eq!(binding.visibility, Visibility::Local);
                assert!(binding.mutable);
                assert_eq!(binding.ty, Ty::B16);
            }
            assert_eq!(var.values.len(), 2);
        }
    }

    #[test]
    fn hoisted_and_inline_parts_combine() {
        let program = clean("var.local [str 'a', b16 'b'] = [|hi|, |1000|];");
        let Stmt::Var(var) = &program.stmts[0] else { panic!("not a declaration") };
        assert_eq!(var.bindings[0].ty, Ty::Str);
        assert_eq!(var.bindings[1].ty, Ty::B16);
        assert!(var.bindings.iter().all(|b| b.visibility == Visibility::Local));
    }

    #[test]
    fn saying_nothing_about_visibility_means_restricted() {
        let program = clean("var.b16 ['x'] = [|1|];");
        let Stmt::Var(var) = &program.stmts[0] else { panic!("not a declaration") };
        assert_eq!(var.bindings[0].visibility, Visibility::Restricted);
        assert_eq!(var.bindings[0].visibility_span, None);
        assert!(!var.bindings[0].mutable);
    }

    #[test]
    fn arithmetic_binds_the_way_mathematics_does() {
        assert_eq!(math("2 + 3 * 4"), "(+ 2 (* 3 4))");
        assert_eq!(math("2 x 3 + 4"), "(+ (* 2 3) 4)");
        assert_eq!(math("(2 + 3) * 4"), "(* (+ 2 3) 4)");
        // Exponent binds tightest.
        assert_eq!(math("2 * 3 ** 4"), "(* 2 (** 3 4))");
        // And groups rightward, so this is 2 ** 9, not 8 ** 2.
        assert_eq!(math("2 ** 3 ** 2"), "(** 2 (** 3 2))");
        // Unary minus sits below the exponent, exactly as -x² is read.
        assert_eq!(math("-2 ** 2"), "(neg (** 2 2))");
        assert_eq!(math("2 ** -3"), "(** 2 (neg 3))");
        // Subtraction and division go leftward.
        assert_eq!(math("10 - 3 - 2"), "(- (- 10 3) 2)");
        assert_eq!(math("100 div 5 div 2"), "(/ (/ 100 5) 2)");
        // Remainder sits with multiplication.
        assert_eq!(math("1 + 7 mod 3"), "(+ 1 (mod 7 3))");
    }

    #[test]
    fn every_spelling_of_an_operator_means_the_same_thing() {
        assert_eq!(math("2 * 3"), math("2 x 3"));
        assert_eq!(math("2 / 3"), math("2 div 3"));
        assert_eq!(math("2 ÷ 3"), math("2 div 3"));
        assert_eq!(math("2 ** 3"), math("2 xx 3"));
        assert_eq!(math("2 ** 3"), math("2 pow 3"));
    }

    #[test]
    fn a_comparison_is_the_loosest_thing_in_a_math_block() {
        assert_eq!(math("'a' + 1 < 'b'"), "(< (+ a 1) b)");
        assert_eq!(math("'a' < 'b' + 1"), "(< a (+ b 1))");
        assert_eq!(math("'a' x 2 = 'b'"), "(= (* a 2) b)");
        assert_eq!(math("'a' > 'b'"), "(> a b)");
        // `=` inside a math block is a comparison; the `=` of a declaration is elsewhere
        // entirely, between a list of names and a list of values, so there is no clash.
        assert_eq!(math("('a' + 1) = ('b' - 1)"), "(= (+ a 1) (- b 1))");
    }

    #[test]
    fn a_chain_of_comparisons_is_refused_rather_than_guessed_at() {
        // The first error is the one that matters; what follows is the parser finding its
        // way out of a math block it was left in the middle of.
        for source in [
            "var.local.bool ['c'] = [math { 'a' < 'b' < 'c' }];",
            "var.local.bool ['c'] = [math { 'a' = 'b' = 'c' }];",
        ] {
            assert_eq!(codes(source).first().map(String::as_str), Some("E0114"), "{source}");
        }
    }

    #[test]
    fn recovering_from_a_bad_statement_always_moves_forward() {
        // A failure inside a math block leaves the cursor at a `}`, which resynchronising
        // deliberately will not eat -- so without forcing progress the parser reported the
        // same error until it ran out of memory. This finishes.
        let out = parse_str("var.local.bool ['c'] = [math { 'a' < 2 < 3 }];\nprint[\"after\"];");
        assert!(out.errors.len() < 5, "{} errors from one bad line", out.errors.len());
        assert_eq!(out.errors[0].code, "E0114");
    }

    #[test]
    fn percent_hangs_off_the_number_in_front_of_it() {
        assert_eq!(math("20%"), "(% 20)");
        assert_eq!(math("'price' x 20%"), "(* price (% 20))");
    }

    #[test]
    fn quotes_mean_a_variable_in_a_math_block_and_a_literal_outside_one() {
        // In the value slot, a literal to be read as b16.
        let program = clean("var.local.b16 ['x'] = [|1000|];");
        let Stmt::Var(var) = &program.stmts[0] else { panic!("not a declaration") };
        assert!(matches!(&var.values[0], Expr::Literal { text, .. } if text == "1000"));

        // Inside math, the same quotes are a variable called `1000`.
        assert_eq!(math("'1000'"), "1000");
        assert_eq!(math("'x' + 1"), "(+ x 1)");
    }

    #[test]
    fn a_declaration_without_a_type_is_reported() {
        assert_eq!(codes("var.local ['x'] = [|1|];"), ["E0105"]);
    }

    #[test]
    fn a_word_that_belongs_to_no_slot_is_reported() {
        let errors = codes("var.local.wobbly.b16 ['x'] = [|1|];");
        assert_eq!(errors, ["E0102"]);
    }

    #[test]
    fn saying_a_thing_twice_is_reported_with_both_places() {
        let out = parse_str("var.local.global.b16 ['x'] = [|1|];");
        assert_eq!(out.errors.len(), 1);
        assert_eq!(out.errors[0].code, "E0103");
        assert_eq!(out.errors[0].labels.len(), 2, "both spellings are pointed at");
    }

    #[test]
    fn a_loop_must_say_how_long_its_counter_lives() {
        assert_eq!(codes("loop.range.ui8 ['i'] = [|1|,|5|] { }"), ["E0108"]);
        assert_eq!(codes("loop.temp.ui8 ['i'] = [|1|,|5|] { }"), ["E0109"]);
        assert_eq!(codes("loop.temp.range ['i'] = [|1|,|5|] { }"), ["E0105"]);
    }

    #[test]
    fn a_statement_that_starts_with_the_wrong_word_is_reported() {
        assert_eq!(codes("wobble 'x';"), ["E0101"]);
        assert_eq!(codes("'x' = [|1|];"), ["E0101"]);
        // With a list after it, it is a call and not a mystery: the parser takes it and
        // the checker is the one that says there is no such function.
        clean("wobble ['x'];");
    }

    #[test]
    fn handback_needs_its_as() {
        assert_eq!(codes("handback 'i' 'total';"), ["E0106"]);
        let program = clean("handback 'i' as 'total';");
        assert!(matches!(&program.stmts[0], Stmt::Handback(h) if h.source.text == "i"));
    }

    #[test]
    fn arithmetic_outside_a_math_block_is_reported_helpfully() {
        let out = parse_str("var.local.b64 ['x'] = [1 + 2];");
        assert_eq!(out.errors[0].code, "E0111");
        assert!(out.errors[0].tips[0].contains("math { }"));
    }

    #[test]
    fn one_bad_statement_does_not_swallow_the_rest() {
        // Three separate problems in one file, all found.
        let source = "var.local ['a'] = [|1|];\n\
                      wobble 'b';\n\
                      var.local.global.b16 ['c'] = [|1|];\n\
                      print['c'];\n";
        let out = parse_str(source);
        assert_eq!(
            out.errors.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            ["E0105", "E0101", "E0103"]
        );
        // And the good statement after them still made it into the tree.
        assert!(matches!(out.program.stmts.last(), Some(Stmt::Print(_))));
    }

    #[test]
    fn defaults_are_a_setting_and_a_behaviour() {
        let program = clean("defaults.no-visibility-stated.error;");
        let Stmt::Defaults(defaults) = &program.stmts[0] else { panic!("not a default") };
        assert_eq!(defaults.setting, "no-visibility-stated");
        assert_eq!(defaults.behaviour, "error");
        assert_eq!(codes("defaults.overflow;"), ["E0110"]);
    }

    #[test]
    fn a_name_may_be_anything_at_all() {
        let program = clean("var.local.b16 ['🧑‍🧑‍🧒‍🧒', 'a friendly greeting'] = [|1|, |2|];");
        let Stmt::Var(var) = &program.stmts[0] else { panic!("not a declaration") };
        assert_eq!(var.bindings[0].name.text, "🧑‍🧑‍🧒‍🧒");
        assert_eq!(var.bindings[1].name.text, "a friendly greeting");
    }

    #[test]
    fn every_node_can_point_at_itself() {
        let source = "var.local.mut.b16 ['x'] = [|1000|];";
        let program = clean(source);
        let span = program.stmts[0].span();
        assert_eq!(&source[span.start..span.end], source);

        let Stmt::Var(var) = &program.stmts[0] else { panic!("not a declaration") };
        let name = var.bindings[0].name.span;
        assert_eq!(&source[name.start..name.end], "'x'");
        let ty = var.bindings[0].ty_span;
        assert_eq!(&source[ty.start..ty.end], "b16");
        let mutable = var.bindings[0].mutable_span.unwrap();
        assert_eq!(&source[mutable.start..mutable.end], "mut");
    }
    #[test]
    fn an_if_takes_its_arms_in_order() {
        let program = clean(
            "if [|true|] { print[\"a\"]; }\n\
             else-if [|false|] { print[\"b\"]; }\n\
             else-if [|false|] { print[\"c\"]; }\n\
             else { print[\"d\"]; }",
        );
        let Stmt::If(if_stmt) = &program.stmts[0] else { panic!("an if") };
        // The `if` and both `else-if`s are the same shape, so they are one list.
        assert_eq!(if_stmt.arms.len(), 3);
        assert!(if_stmt.otherwise.is_some());
    }

    #[test]
    fn an_if_needs_neither_an_else_nor_an_else_if() {
        let program = clean("if [|true|] { print[\"a\"]; }");
        let Stmt::If(if_stmt) = &program.stmts[0] else { panic!("an if") };
        assert_eq!(if_stmt.arms.len(), 1);
        assert!(if_stmt.otherwise.is_none());
    }

    #[test]
    fn nothing_may_follow_the_else() {
        assert_eq!(
            codes("if [|true|] { } else { } else-if [|true|] { }"),
            ["E0117"]
        );
        assert_eq!(codes("if [|true|] { } else { } else { }"), ["E0118"]);
    }

    #[test]
    fn or_is_looser_than_and_which_is_looser_than_not() {
        // `a or b and c` is `a or (b and c)`, and `not a and b` is `(not a) and b`.
        assert_eq!(math("'a' or 'b' and 'c'"), "(or a (and b c))");
        assert_eq!(math("not 'a' and 'b'"), "(and (not a) b)");
        // And `not` is looser than a comparison, so it turns the whole comparison around
        // rather than one side of it.
        assert_eq!(math("not 'a' = 'b'"), "(not (= a b))");
    }

    #[test]
    fn the_two_nots_do_not_collide() {
        // `not` before something turns it around; `not` before `=` is a way of spelling
        // `!=`, and it can only ever appear after something.
        assert_eq!(math("'a' not= 'b'"), "(!= a b)");
        assert_eq!(math("not 'a' not= 'b'"), "(not (!= a b))");
    }

    #[test]
    fn a_function_says_who_sees_it_and_what_it_answers() {
        let program = clean(
            "fn.local.i32 ['bigger'] [i32 'a', i32 'b'] {\n\
                 if [math { 'a' > 'b' }] { return 'a'; }\n\
                 return 'b';\n\
             }",
        );
        let Stmt::Func(func) = &program.stmts[0] else { panic!("a function") };
        assert_eq!(func.name.text, "bigger");
        assert_eq!(func.visibility, Visibility::Local);
        assert_eq!(func.returns, Some(Ty::I32));
        assert_eq!(func.params.len(), 2);
        assert_eq!(func.params[0].name.text, "a");
        assert_eq!(func.params[1].ty, Ty::I32);
    }

    #[test]
    fn a_function_may_answer_nothing_but_must_say_so() {
        let program = clean("fn.local.nothing ['greet'] [str 'who'] { print['who']; }");
        let Stmt::Func(func) = &program.stmts[0] else { panic!("a function") };
        assert_eq!(func.returns, None);
        // Leaving it unsaid is not the same as saying nothing.
        assert_eq!(codes("fn.local ['greet'] [str 'who'] { }"), ["E0121"]);
    }

    #[test]
    fn a_function_may_take_nothing_at_all() {
        let program = clean("fn.local.i32 ['answer'] [] { return |42|; }");
        let Stmt::Func(func) = &program.stmts[0] else { panic!("a function") };
        assert!(func.params.is_empty());
    }

    #[test]
    fn a_bare_word_before_a_list_is_a_call_and_a_quoted_one_is_not() {
        // `double[…]` calls; `'double'` would be a variable of that name.
        assert_eq!(math("double[|5|]"), "(double |5|)");
        assert_eq!(math("'a' + double[|5|, 'b']"), "(+ a (double |5| b))");
        // And a call is a value in its own right, not only a math term.
        let program = clean("var.local.i32 ['n'] = [double[|5|]];");
        let Stmt::Var(var) = &program.stmts[0] else { panic!("a declaration") };
        assert!(matches!(&var.values[0], Expr::Call { .. }));
    }

    #[test]
    fn return_may_carry_a_value_or_nothing() {
        let program = clean("fn.local.nothing ['f'] [] { return; }");
        let Stmt::Func(func) = &program.stmts[0] else { panic!("a function") };
        let Stmt::Return(ret) = &func.body[0] else { panic!("a return") };
        assert!(ret.value.is_none());
    }

    #[test]
    fn a_while_loop_counts_its_passes_only_if_it_asks_to() {
        let program = clean("loop.while [|true|] { print[\"x\"]; }");
        let Stmt::While(while_stmt) = &program.stmts[0] else { panic!("a while loop") };
        assert!(while_stmt.counter.is_none());

        let program = clean("loop.temp.while.ui32 ['n'] [|true|] { print['n']; }");
        let Stmt::While(while_stmt) = &program.stmts[0] else { panic!("a while loop") };
        let counter = while_stmt.counter.as_ref().expect("it named one");
        assert_eq!(counter.name.text, "n");
        assert_eq!(counter.ty, Ty::U32);
        assert_eq!(counter.lifetime, Lifetime::Temp);
    }

    #[test]
    fn a_while_loop_with_no_counter_has_no_lifetime_to_state() {
        // `temp` and `perm` are about a counter, and this one has none.
        assert_eq!(codes("loop.temp.while [|true|] { }"), ["E0131"]);
        // But one with a counter must say.
        assert_eq!(codes("loop.while.ui32 ['n'] [|true|] { }"), ["E0108"]);
    }

    #[test]
    fn break_either_stops_or_says_when() {
        let program = clean("loop.while [|true|] { break; }");
        let Stmt::While(while_stmt) = &program.stmts[0] else { panic!("a while loop") };
        assert!(matches!(&while_stmt.body[0], Stmt::Break(b) if b.reached.is_none()));

        let program = clean("loop.temp.while.ui8 ['n'] [|true|] { break when reached |3|; }");
        let Stmt::While(while_stmt) = &program.stmts[0] else { panic!("a while loop") };
        assert!(matches!(&while_stmt.body[0], Stmt::Break(b) if b.reached.is_some()));

        // Anything else after `break` is a mistake with a name.
        assert_eq!(codes("loop.while [|true|] { break |3|; }"), ["E0132"]);
        assert_eq!(codes("loop.while [|true|] { break when |3|; }"), ["E0132"]);
    }

}

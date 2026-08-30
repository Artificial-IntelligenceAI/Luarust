//! Turning Luarust source into tokens.
//!
//! Luarust settles more at this stage than most languages do, because its punctuation is
//! unambiguous by design. Five brackets that never share a meaning, and two kinds of
//! quote that tell a name apart from text — so by the time the parser sees anything, it
//! already knows whether `'x'` was a variable or `"x"` was a word of English.
//!
//! Two things this deliberately does **not** do. It does not decide which bare words are
//! keywords, because `error` means something in `defaults.no-visibility-stated.error` and
//! nothing anywhere else, and reserving it everywhere would be a cost the language never
//! has to pay. And it does not work out what a number is worth, because `'1000'` is a
//! different value under `b16` than under `i32` and nothing here knows which is wanted
//! yet.

pub mod token;

use luarust_diag::{Diagnostic, Span};
pub use token::{Kind, Token};

/// The result of reading a file: the tokens, and whatever was wrong with it.
///
/// Lexing never stops at the first mistake. An unterminated string or a stray character
/// is reported and skipped, so that one bad line does not hide the twelve after it.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<Diagnostic>,
}

impl Lexed {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Read a whole source file.
pub fn lex(source: &str) -> Lexed {
    Lexer { source, at: 0, tokens: Vec::new(), errors: Vec::new() }.run()
}

/// The escapes that may be written, both bare and inside text.
fn escape_value(c: char) -> Option<char> {
    match c {
        'n' => Some('\n'),
        't' => Some('\t'),
        'r' => Some('\r'),
        '0' => Some('\0'),
        '\\' => Some('\\'),
        _ => None,
    }
}

/// Resolve the escapes in a [`Kind::Text`] token's source, quotes included.
///
/// The lexer has already checked that every escape here is one it knows, so anything
/// unrecognised at this point is a bug rather than a bad program.
pub fn text_value(raw: &str) -> String {
    let inner = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(raw);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next().and_then(escape_value) {
                Some(resolved) => out.push(resolved),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The text between the quotes of a [`Kind::Name`] token.
///
/// Names are raw: whatever is between the quotes is the name, so it can hold spaces,
/// punctuation and emoji without any of it meaning anything.
pub fn name_value(raw: &str) -> &str {
    raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).unwrap_or(raw)
}

struct Lexer<'a> {
    source: &'a str,
    at: usize,
    tokens: Vec<Token>,
    errors: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn run(mut self) -> Lexed {
        while let Some(c) = self.peek() {
            match c {
                c if c.is_whitespace() => self.bump(),

                // `--` runs to the end of the line. A lone `-` is subtraction.
                '-' if self.peek_at(1) == Some('-') => self.skip_comment(),

                '[' => self.punctuation(Kind::OpenList),
                ']' => self.punctuation(Kind::CloseList),
                '{' => self.punctuation(Kind::OpenBlock),
                '}' => self.punctuation(Kind::CloseBlock),
                '(' => self.punctuation(Kind::OpenGroup),
                ')' => self.punctuation(Kind::CloseGroup),
                ';' => self.punctuation(Kind::Semicolon),
                ',' => self.punctuation(Kind::Comma),
                '.' => self.punctuation(Kind::Dot),
                '=' => self.punctuation(Kind::Equals),
                '+' => self.punctuation(Kind::Plus),
                '-' => self.punctuation(Kind::Minus),
                '/' | '÷' => self.punctuation(Kind::Slash),
                '%' => self.punctuation(Kind::Percent),
                // `</=` and `>/=` are one operator each: the `/` is the "or" in "less
                // than or equal", not a division. Nothing else could follow a `<` with a
                // `/`, so taking all three characters is never wrong. `<=` and `≤` are the
                // same operator written the usual way and the mathematical way.
                '<' => self.or_equal(Kind::LessEqual, Kind::Less),
                '>' => self.or_equal(Kind::GreaterEqual, Kind::Greater),
                '≤' => self.punctuation(Kind::LessEqual),
                '≥' => self.punctuation(Kind::GreaterEqual),
                '≠' => self.punctuation(Kind::NotEqual),
                '!' => {
                    let start = self.at;
                    self.bump();
                    if self.peek() == Some('=') {
                        self.bump();
                        self.push(Kind::NotEqual, start);
                    } else {
                        self.at = start;
                        self.unknown_character();
                    }
                }

                '*' => {
                    let start = self.at;
                    self.bump();
                    let kind = if self.peek() == Some('*') {
                        self.bump();
                        Kind::StarStar
                    } else {
                        Kind::Star
                    };
                    self.push(kind, start);
                }

                '\'' => self.quoted('\'', Kind::Name),
                '"' => self.quoted('"', Kind::Text),
                '\\' => self.bare_escape(),

                c if c.is_ascii_digit() => self.number(),
                c if is_word_start(c) => self.word(),

                _ => self.unknown_character(),
            }
        }

        let end = Span::at(self.source.len());
        self.tokens.push(Token { kind: Kind::End, span: end });
        Lexed { tokens: self.tokens, errors: self.errors }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.at..].chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.source[self.at..].chars().nth(n)
    }

    fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.at += c.len_utf8();
        }
    }

    fn push(&mut self, kind: Kind, start: usize) {
        self.tokens.push(Token { kind, span: Span::new(start, self.at) });
    }

    fn punctuation(&mut self, kind: Kind) {
        let start = self.at;
        self.bump();
        self.push(kind, start);
    }

    /// `<` or `>`, and whether it is the `</=` or `>/=` that includes being equal.
    fn or_equal(&mut self, both: Kind, plain: Kind) {
        let start = self.at;
        self.bump();
        match (self.peek(), self.peek_at(1)) {
            (Some('='), _) => {
                self.bump();
                self.push(both, start);
            }
            (Some('/'), Some('=')) => {
                self.bump();
                self.bump();
                self.push(both, start);
            }
            _ => self.push(plain, start),
        }
    }

    fn skip_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    /// A name or a piece of text, from one quote to the matching one.
    fn quoted(&mut self, quote: char, kind: Kind) {
        let start = self.at;
        self.bump(); // the opening quote

        while let Some(c) = self.peek() {
            match c {
                // A closing quote on the same line ends it.
                c if c == quote => {
                    self.bump();
                    self.push(kind, start);
                    return;
                }
                // Only text has escapes; a name is raw, so it can hold a backslash.
                '\\' if kind == Kind::Text => {
                    let escape_at = self.at;
                    self.bump();
                    match self.peek() {
                        Some(e) if escape_value(e).is_some() => self.bump(),
                        _ => {
                            let end = self.peek().map_or(self.at, |e| self.at + e.len_utf8());
                            self.bump();
                            self.unknown_escape(Span::new(escape_at, end));
                        }
                    }
                }
                // A quote is not allowed to run past the end of its line: the far more
                // likely reading is a missing closing quote, not a name with a newline in.
                '\n' => break,
                _ => self.bump(),
            }
        }

        let span = Span::new(start, self.at);
        let (code, what) = match kind {
            Kind::Text => ("E0001", "text"),
            _ => ("E0002", "a name"),
        };
        self.errors.push(
            Diagnostic::new(code, format!("this {what} is opened and never closed."))
                .primary(span, format!("{what} starts here"))
                .rule(format!("{what} opens and closes on the same line"))
                .tip(format!(
                    "a `{quote}` inside {what} would end it early, which may be what happened."
                ))
                .fix(format!("add a closing `{quote}` before the end of the line.")),
        );
        self.push(kind, start);
    }

    /// `\n` and friends, written outside the quotes.
    fn bare_escape(&mut self) {
        let start = self.at;
        self.bump(); // the backslash
        match self.peek() {
            Some(c) if escape_value(c).is_some() => {
                self.bump();
                self.push(Kind::Escape, start);
            }
            _ => {
                let end = self.peek().map_or(self.at, |c| self.at + c.len_utf8());
                self.bump();
                self.unknown_escape(Span::new(start, end));
            }
        }
    }

    fn number(&mut self) {
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        // A dot is only part of the number when a digit follows it, so that `1.5` is one
        // number while the `.` of a chain stays a `.`.
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        self.push(Kind::Number, start);
    }

    fn word(&mut self) {
        let start = self.at;
        self.bump();
        loop {
            match self.peek() {
                Some(c) if is_word_continue(c) => self.bump(),
                // A hyphen belongs to the word only when a letter follows it, so
                // `no-visibility-stated` is one word while `'a' mod-3` is not.
                Some('-') if self.peek_at(1).is_some_and(|c| c.is_alphabetic()) => {
                    self.bump();
                    self.bump();
                }
                _ => break,
            }
        }
        self.push(Kind::Word, start);
    }

    fn unknown_character(&mut self) {
        let start = self.at;
        let c = self.peek().unwrap_or('\0');
        self.bump();
        let span = Span::new(start, self.at);
        self.errors.push(
            Diagnostic::new("E0003", format!("`{c}` does not mean anything here."))
                .primary(span, "this character")
                .rule("every character outside quotes is punctuation, a word, or a number")
                .tip("a name goes in single quotes and text goes in double quotes, so anything unusual belongs inside a pair of them.")
                .fix(format!("delete it, or quote it: `'{c}'` for a name, `\"{c}\"` for text.")),
        );
    }

    fn unknown_escape(&mut self, span: Span) {
        self.errors.push(
            Diagnostic::new("E0004", "that is not an escape Luarust knows.")
                .primary(span, "written here")
                .rule("an escape is one of `\\n`, `\\t`, `\\r`, `\\0` or `\\\\`")
                .fix("write `\\\\` if a backslash of its own was meant."),
        );
    }
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use luarust_diag::SourceFile;

    fn kinds(source: &str) -> Vec<Kind> {
        lex(source).tokens.iter().map(|t| t.kind).collect()
    }

    fn texts(source: &str) -> Vec<&str> {
        lex(source)
            .tokens
            .iter()
            .filter(|t| t.kind != Kind::End)
            .map(|t| &source[t.span.start..t.span.end])
            .collect()
    }

    fn clean(source: &str) -> Lexed {
        let out = lex(source);
        assert!(out.ok(), "expected no errors, got {:#?}", out.errors);
        out
    }

    #[test]
    fn the_readme_loop_lexes_the_way_it_reads() {
        use Kind::*;
        let source = "loop.temp.range.ui8 ['i'] = ['1', '5'] {\n    print['i' \\n];\n}\n";
        clean(source);
        assert_eq!(
            kinds(source),
            [
                Word, Dot, Word, Dot, Word, Dot, Word,
                OpenList, Name, CloseList,
                Equals,
                OpenList, Name, Comma, Name, CloseList,
                OpenBlock,
                Word, OpenList, Name, Escape, CloseList, Semicolon,
                CloseBlock,
                End,
            ]
        );
    }

    #[test]
    fn a_math_block_lexes_its_operators() {
        use Kind::*;
        let source = "var.local.b16 ['z'] = [math { ('x' + 'y') * 'x' }];";
        clean(source);
        let k = kinds(source);
        assert!(k.contains(&OpenGroup) && k.contains(&CloseGroup));
        assert_eq!(k[k.len() - 4..], [CloseBlock, CloseList, Semicolon, End]);
        assert_eq!(
            k.iter().filter(|k| **k == Name).count(),
            4,
            "'z', 'x', 'y' and 'x' again"
        );
    }

    #[test]
    fn a_bare_number_is_a_number_and_a_quoted_one_is_a_name() {
        assert_eq!(kinds("math { 'x' + 1 }")[..], [
            Kind::Word, Kind::OpenBlock, Kind::Name, Kind::Plus, Kind::Number, Kind::CloseBlock,
            Kind::End
        ]);
        // Quoted, it is a name -- a variable called `1` -- which is exactly what the
        // README says the quotes now mean inside a math block.
        assert_eq!(kinds("math { 'x' + '1' }")[4], Kind::Name);
    }

    #[test]
    fn numbers_keep_their_decimals_but_not_a_chain_dot() {
        assert_eq!(texts("0.1"), ["0.1"]);
        assert_eq!(texts("1000000007"), ["1000000007"]);
        // The dot of a chain is not swallowed by the number in front of it.
        assert_eq!(kinds("1.abc")[..3], [Kind::Number, Kind::Dot, Kind::Word]);
        assert_eq!(texts("1.abc"), ["1", ".", "abc"]);
    }

    #[test]
    fn star_and_star_star_are_told_apart() {
        assert_eq!(kinds("* ** *")[..3], [Kind::Star, Kind::StarStar, Kind::Star]);
    }

    #[test]
    fn division_may_be_spelled_with_either_sign() {
        assert_eq!(kinds("/")[0], Kind::Slash);
        assert_eq!(kinds("÷")[0], Kind::Slash);
        // And the multi-byte one still spans exactly itself.
        let out = clean("÷");
        assert_eq!(out.tokens[0].span, Span::new(0, "÷".len()));
    }

    #[test]
    fn or_equal_has_three_spellings_each() {
        for source in ["</=", "<=", "≤"] {
            assert_eq!(kinds(source)[..2], [Kind::LessEqual, Kind::End], "{source}");
            assert_eq!(clean(source).tokens[0].span, Span::new(0, source.len()));
        }
        for source in [">/=", ">=", "≥"] {
            assert_eq!(kinds(source)[..2], [Kind::GreaterEqual, Kind::End], "{source}");
        }
        // A bare `<` is still a bare `<`, and the `=` after it is still an `=`.
        assert_eq!(kinds("< =")[..3], [Kind::Less, Kind::Equals, Kind::End]);
        assert_eq!(kinds("'a' < 'b'")[1], Kind::Less);
    }

    #[test]
    fn percent_is_its_own_thing_and_never_a_remainder() {
        assert_eq!(kinds("20%")[..2], [Kind::Number, Kind::Percent]);
        // `mod` is a word, not punctuation.
        assert_eq!(kinds("mod")[0], Kind::Word);
    }

    #[test]
    fn a_hyphen_joins_a_word_but_does_not_swallow_a_number() {
        assert_eq!(texts("defaults.no-visibility-stated.error;"), [
            "defaults", ".", "no-visibility-stated", ".", "error", ";"
        ]);
        // Subtraction survives next to a word.
        assert_eq!(kinds("mod-3")[..3], [Kind::Word, Kind::Minus, Kind::Number]);
        assert_eq!(kinds("'a' - 'b'")[1], Kind::Minus);
    }

    #[test]
    fn a_name_holds_whatever_you_put_in_it() {
        let source = "var.local.b16 ['🧑‍🧑‍🧒‍🧒'] = ['1'];";
        clean(source);
        let out = lex(source);
        let name = out.tokens.iter().find(|t| t.kind == Kind::Name).unwrap();
        assert_eq!(name_value(&source[name.span.start..name.span.end]), "🧑‍🧑‍🧒‍🧒");

        // Spaces and punctuation too, since none of it means anything in there.
        let source = "var.local.str ['a friendly, greeting'] = ['hi'];";
        clean(source);
        let names: Vec<&str> = texts(source).into_iter().filter(|t| t.starts_with('\'')).collect();
        assert_eq!(names, ["'a friendly, greeting'", "'hi'"]);
    }

    #[test]
    fn comments_run_to_the_end_of_the_line() {
        let source = "var.local.b16 ['x'] = ['1'];  -- the number one\nprint['x'];";
        clean(source);
        assert!(!texts(source).iter().any(|t| t.contains("number one")));
        assert_eq!(kinds(source).iter().filter(|k| **k == Kind::Semicolon).count(), 2);
        // A comment at the very end of a file needs no newline after it.
        clean("-- nothing but a remark");
        assert_eq!(kinds("-- nothing but a remark"), [Kind::End]);
    }

    #[test]
    fn escapes_work_bare_and_inside_text() {
        let source = "print[\"take \\t care\\n\" 'x' \\n];";
        clean(source);
        let out = lex(source);
        let text = out.tokens.iter().find(|t| t.kind == Kind::Text).unwrap();
        assert_eq!(
            text_value(&source[text.span.start..text.span.end]),
            "take \t care\n"
        );
        assert_eq!(out.tokens.iter().filter(|t| t.kind == Kind::Escape).count(), 1);
    }

    #[test]
    fn a_name_keeps_its_backslash_because_it_is_raw() {
        let source = r"var.local.str ['back\slash'] = ['x'];";
        clean(source);
        let out = lex(source);
        let name = out.tokens.iter().find(|t| t.kind == Kind::Name).unwrap();
        assert_eq!(name_value(&source[name.span.start..name.span.end]), r"back\slash");
    }

    #[test]
    fn an_unclosed_text_is_reported_and_does_not_eat_the_file() {
        let source = "print[\"never closed\nprint['x'];";
        let out = lex(source);
        assert_eq!(out.errors.len(), 1);
        assert_eq!(out.errors[0].code, "E0001");
        // The second line still lexed, so the next error would be found too.
        assert!(out.tokens.iter().any(|t| t.kind == Kind::Semicolon));
    }

    #[test]
    fn an_unclosed_name_is_reported_separately() {
        let out = lex("var.local.str ['name = ['x'];");
        assert_eq!(out.errors.len(), 1);
        assert_eq!(out.errors[0].code, "E0002");
    }

    #[test]
    fn a_stray_character_is_reported_and_stepped_over() {
        let out = lex("var @ local");
        assert_eq!(out.errors.len(), 1);
        assert_eq!(out.errors[0].code, "E0003");
        assert_eq!(
            out.tokens.iter().filter(|t| t.kind == Kind::Word).count(),
            2,
            "`var` and `local` both survived"
        );
    }

    #[test]
    fn an_unknown_escape_is_reported_bare_or_in_text() {
        let bare = lex("print['x' \\q];");
        assert_eq!(bare.errors.len(), 1);
        assert_eq!(bare.errors[0].code, "E0004");

        let inside = lex("print[\"oops \\q\"];");
        assert_eq!(inside.errors.len(), 1);
        assert_eq!(inside.errors[0].code, "E0004");
    }

    #[test]
    fn every_mistake_in_a_file_is_found_at_once() {
        // The point of not stopping: three problems, three reports.
        let out = lex("var @ local;\nprint[\"unclosed\nprint['x' \\q];");
        let codes: Vec<&str> = out.errors.iter().map(|e| e.code.as_str()).collect();
        assert_eq!(codes, ["E0003", "E0001", "E0004"]);
    }

    #[test]
    fn a_lexing_error_renders_the_way_every_error_does() {
        let source = "var @ local";
        let out = lex(source);
        let file = SourceFile::new("src/main.lr", source);
        let rendered = luarust_diag::report(&file, &out.errors);
        assert!(rendered.contains("`@` does not mean anything here."), "{rendered}");
        assert!(rendered.contains("file: src/main.lr, line: 1, column: 5"), "{rendered}");
        assert!(rendered.contains("Error code: E0003"), "{rendered}");
        assert!(rendered.ends_with("1 error.\n"), "{rendered}");
    }

    #[test]
    fn every_token_spans_exactly_itself() {
        let source = "loop.temp.range.ui8 ['i'] = ['1', '5'] { print['i' \\n]; }";
        let out = clean(source);
        let mut previous_end = 0;
        for token in &out.tokens {
            assert!(token.span.start >= previous_end, "tokens overlap");
            assert!(token.span.end <= source.len());
            assert!(source.is_char_boundary(token.span.start));
            assert!(source.is_char_boundary(token.span.end));
            previous_end = token.span.end;
        }
        assert_eq!(out.tokens.last().unwrap().kind, Kind::End);
    }

    #[test]
    fn an_empty_file_still_ends() {
        let out = clean("");
        assert_eq!(out.tokens.len(), 1);
        assert_eq!(out.tokens[0].kind, Kind::End);
        clean("   \n\n\t  ");
    }
}

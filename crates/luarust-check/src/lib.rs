//! Names, types, visibility and mutability.
//!
//! Everything Luarust refuses to guess is refused here. A name has to have been declared;
//! a type has to match rather than convert; a variable has to have said `mut` before it
//! can be changed; a literal has to fit the type it is being read as; and a `restricted`
//! variable — which is what a declaration means when it says nothing about visibility —
//! cannot be touched at all.
//!
//! What comes out has no names left in it. Every variable is a numbered slot and every
//! written literal is already a value of a known type, so nothing downstream has to work
//! any of it out again.

pub mod ir;
pub mod value;

use ir::Checked;
use luarust_diag::{Diagnostic, Span};
use luarust_num::binary::{self, Round};
use luarust_parse::ast::{
    self, BinOp, CmpOp, Expr as AExpr, Lifetime, PrintItem, Stmt as AStmt, Ty, Visibility,
};
use std::collections::HashMap;
use value::{Overflow, Value, format_of};

/// What is known about one declared variable.
#[derive(Clone, Debug)]
struct Var {
    slot: usize,
    ty: Ty,
    mutable: bool,
    visibility: Visibility,
    declared_at: Span,
    /// Where its visibility was written, when it was.
    visibility_at: Option<Span>,
}

/// Check a parsed program.
pub fn check(program: &ast::Program) -> (Checked, Vec<Diagnostic>) {
    let mut checker = Checker {
        scopes: vec![HashMap::new()],
        slots: 0,
        overflow: Overflow::Wrap,
        visibility_required: false,
        errors: Vec::new(),
    };

    // Defaults are read first, wherever in the file they were written, so that a
    // declaration above one still answers to it.
    for stmt in &program.stmts {
        if let AStmt::Defaults(defaults) = stmt {
            checker.apply_default(defaults);
        }
    }

    let stmts = checker.block(&program.stmts);
    let checked = Checked { stmts, slots: checker.slots, overflow: checker.overflow };
    (checked, checker.errors)
}

struct Checker {
    scopes: Vec<HashMap<String, Var>>,
    slots: usize,
    overflow: Overflow,
    visibility_required: bool,
    errors: Vec<Diagnostic>,
}

impl Checker {
    fn error(&mut self, diagnostic: Diagnostic) {
        self.errors.push(diagnostic);
    }

    fn lookup(&self, name: &str) -> Option<&Var> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    /// A name close enough to the one written that it was probably meant.
    fn nearest(&self, name: &str) -> Option<String> {
        self.scopes
            .iter()
            .flat_map(|scope| scope.keys())
            .filter(|known| edit_distance(known, name) <= 2.max(name.chars().count() / 3))
            .min_by_key(|known| edit_distance(known, name))
            .cloned()
    }

    fn declare(&mut self, name: &str, var: Var) -> usize {
        let slot = var.slot;
        self.scopes.last_mut().expect("a scope is always open").insert(name.to_string(), var);
        slot
    }

    fn next_slot(&mut self) -> usize {
        self.slots += 1;
        self.slots - 1
    }

    fn apply_default(&mut self, defaults: &ast::Defaults) {
        match (defaults.setting.as_str(), defaults.behaviour.as_str()) {
            ("no-visibility-stated", "error") => self.visibility_required = true,
            ("no-visibility-stated", "restricted") => self.visibility_required = false,
            ("overflow", "trap") => self.overflow = Overflow::Trap,
            ("overflow", "wrap") => self.overflow = Overflow::Wrap,
            ("no-visibility-stated" | "overflow", behaviour) => self.error(
                Diagnostic::new("E0200", format!("`{behaviour}` is not something `{}` can be set to.", defaults.setting))
                    .primary(defaults.behaviour_span, "written here")
                    .rule("a default is set to one of the behaviours its setting allows")
                    .tip(if defaults.setting == "overflow" {
                        "`overflow` may be `wrap` or `trap`."
                    } else {
                        "`no-visibility-stated` may be `restricted` or `error`."
                    })
                    .fix("use one of those."),
            ),
            (setting, _) => self.error(
                Diagnostic::new("E0201", format!("`{setting}` is not a setting Luarust has."))
                    .primary(defaults.setting_span, "written here")
                    .rule("a default names a setting the language knows")
                    .tip("the settings are `no-visibility-stated` and `overflow`.")
                    .fix("use one of those."),
            ),
        }
    }

    fn block(&mut self, stmts: &[AStmt]) -> Vec<ir::Stmt> {
        let mut out = Vec::new();
        for stmt in stmts {
            match stmt {
                AStmt::Var(var) => out.extend(self.var_decl(var)),
                AStmt::Set(set) => out.extend(self.set_stmt(set)),
                AStmt::Handback(handback) => out.extend(self.handback_stmt(handback)),
                AStmt::Print(print) => out.push(self.print_stmt(print)),
                AStmt::Loop(loop_stmt) => {
                    if let Some(checked) = self.loop_stmt(loop_stmt) {
                        out.push(checked);
                    }
                }
                // Already read, before anything else ran.
                AStmt::Defaults(_) => {}
            }
        }
        out
    }

    fn var_decl(&mut self, var: &ast::Var) -> Vec<ir::Stmt> {
        if !self.counts_match(var.bindings.len(), var.values.len(), var.names_span, var.values_span) {
            return Vec::new();
        }

        let mut out = Vec::new();
        for (binding, value) in var.bindings.iter().zip(&var.values) {
            if !self.usable_type(binding.ty, binding.ty_span) {
                continue;
            }
            if self.visibility_required && binding.visibility_span.is_none() {
                self.error(
                    Diagnostic::new("E0202", format!("`'{}'` does not say who can see it.", binding.name.text))
                        .primary(binding.name.span, "declared here, with no visibility")
                        .rule("with `defaults.no-visibility-stated.error`, every declaration names a visibility")
                        .tip("without one a variable is `restricted`, which means nothing may touch it.")
                        .fix(format!("write `var.local.{} ['{}'] = …;`", binding.ty.word(), binding.name.text)),
                );
                continue;
            }
            if let Some(existing) = self.scopes.last().and_then(|scope| scope.get(&binding.name.text)).cloned() {
                self.error(
                    Diagnostic::new("E0203", format!("`'{}'` is declared twice here.", binding.name.text))
                        .secondary(existing.declared_at, "already declared here")
                        .primary(binding.name.span, "and declared again here")
                        .rule("a name is declared once in a block")
                        .tip("`set` changes a variable that already exists.")
                        .fix(format!("rename this one, or change the first with `set ['{}'] = …;`", binding.name.text)),
                );
                continue;
            }

            let Some(value) = self.expr(value, Some(binding.ty)) else { continue };
            let slot = self.next_slot();
            self.declare(
                &binding.name.text,
                Var {
                    slot,
                    ty: binding.ty,
                    mutable: binding.mutable,
                    visibility: binding.visibility,
                    declared_at: binding.name.span,
                    visibility_at: binding.visibility_span,
                },
            );
            out.push(ir::Stmt::Store { slot, value, span: binding.span });
        }
        out
    }

    fn set_stmt(&mut self, set: &ast::Set) -> Vec<ir::Stmt> {
        if !self.counts_match(set.targets.len(), set.values.len(), set.names_span, set.values_span) {
            return Vec::new();
        }

        let mut out = Vec::new();
        for (target, value) in set.targets.iter().zip(&set.values) {
            let Some(var) = self.resolve(target) else { continue };
            if !self.changeable(&var, target.span) {
                continue;
            }
            let Some(value) = self.expr(value, Some(var.ty)) else { continue };
            out.push(ir::Stmt::Store { slot: var.slot, value, span: target.span });
        }
        out
    }

    fn handback_stmt(&mut self, handback: &ast::Handback) -> Vec<ir::Stmt> {
        let Some(source) = self.resolve(&handback.source) else { return Vec::new() };
        let Some(target) = self.resolve(&handback.target) else { return Vec::new() };
        if !self.changeable(&target, handback.target.span) {
            return Vec::new();
        }
        if source.ty != target.ty {
            self.error(
                Diagnostic::new("E0204", format!(
                    "`'{}'` is `{}` and `'{}'` is `{}`, so one cannot be added to the other.",
                    handback.source.text, source.ty.word(), handback.target.text, target.ty.word()
                ))
                .secondary(source.declared_at, format!("this is `{}`", source.ty.word()))
                .primary(handback.span, "added here")
                .rule("arithmetic works on two numbers of the same type, and nothing converts on its own")
                .fix(format!("declare both as `{}`.", target.ty.word())),
            );
            return Vec::new();
        }

        // `handback 'a' as 'b'` is `set ['b'] = [math { 'b' + 'a' }]`, said shorter.
        let sum = ir::Expr::Binary {
            op: BinOp::Add,
            ty: target.ty,
            lhs: Box::new(ir::Expr::Load { slot: target.slot, ty: target.ty, span: handback.target.span }),
            rhs: Box::new(ir::Expr::Load { slot: source.slot, ty: source.ty, span: handback.source.span }),
            span: handback.span,
        };
        vec![ir::Stmt::Store { slot: target.slot, value: sum, span: handback.span }]
    }

    fn print_stmt(&mut self, print: &ast::Print) -> ir::Stmt {
        let mut items = Vec::new();
        for item in &print.items {
            match item {
                PrintItem::Text { value, .. } => items.push(ir::Item::Text(value.clone())),
                PrintItem::Escape { value, .. } => items.push(ir::Item::Text(value.to_string())),
                PrintItem::Value(expr) => {
                    // Nothing here says what type to read a value as, so print takes
                    // whatever the expression turns out to be.
                    if let Some(checked) = self.expr(expr, None) {
                        items.push(ir::Item::Value(checked));
                    }
                }
            }
        }
        ir::Stmt::Print { items, span: print.span }
    }

    fn loop_stmt(&mut self, loop_stmt: &ast::Loop) -> Option<ir::Stmt> {
        if !self.usable_type(loop_stmt.ty, loop_stmt.ty_span) {
            return None;
        }
        if !loop_stmt.ty.is_integer() && !loop_stmt.ty.is_float() {
            self.error(
                Diagnostic::new("E0205", format!("a loop cannot count in `{}`.", loop_stmt.ty.word()))
                    .primary(loop_stmt.ty_span, "written here")
                    .rule("a loop counts, so its counter is a number")
                    .fix("give the counter a numeric type."),
            );
            return None;
        }

        let from = self.expr(&loop_stmt.from, Some(loop_stmt.ty))?;
        let to = self.expr(&loop_stmt.to, Some(loop_stmt.ty))?;

        // A `perm` counter is declared where the loop is, so it is still there after it.
        // A `temp` one belongs to the body alone.
        let slot = self.next_slot();
        let counter = Var {
            slot,
            ty: loop_stmt.ty,
            mutable: false,
            visibility: Visibility::Local,
            declared_at: loop_stmt.counter.span,
            visibility_at: Some(loop_stmt.lifetime_span),
        };
        if loop_stmt.lifetime == Lifetime::Perm {
            self.declare(&loop_stmt.counter.text, counter.clone());
        }

        self.scopes.push(HashMap::new());
        if loop_stmt.lifetime == Lifetime::Temp {
            self.declare(&loop_stmt.counter.text, counter);
        }
        let body = self.block(&loop_stmt.body);
        self.scopes.pop();

        Some(ir::Stmt::Loop { slot, ty: loop_stmt.ty, from, to, body, span: loop_stmt.span })
    }

    // ---- the parts every statement shares ---------------------------------------

    fn counts_match(&mut self, names: usize, values: usize, names_span: Span, values_span: Span) -> bool {
        if names == values {
            return true;
        }
        let (more, fewer) = if names > values { ("names", "values") } else { ("values", "names") };
        self.error(
            Diagnostic::new("E0206", format!("there are {names} names here and {values} values."))
                .secondary(names_span, format!("{names} names"))
                .primary(values_span, format!("{values} values"))
                .rule("a list of names and a list of values are the same length")
                .tip(format!("there are more {more} than {fewer}, so at least one of them has no partner."))
                .fix("add or remove one, so the two lists match."),
        );
        false
    }

    fn resolve(&mut self, name: &ast::Ident) -> Option<Var> {
        match self.lookup(&name.text) {
            Some(var) => {
                let var = var.clone();
                if var.visibility == Visibility::Restricted {
                    let declared = var.visibility_at.is_some();
                    self.error(
                        Diagnostic::new("E0207", format!("`'{}'` is restricted, so nothing may touch it.", name.text))
                            .secondary(var.declared_at, if declared {
                                "declared `restricted` here"
                            } else {
                                "declared here, and said nothing about who can see it"
                            })
                            .primary(name.span, "used here")
                            .rule("a `restricted` variable exists and may not be used")
                            .tip("saying nothing about visibility makes a variable `restricted`, which is the joke.")
                            .fix(format!("give it a visibility: `var.local.{} ['{}'] = …;`", var.ty.word(), name.text)),
                    );
                    return None;
                }
                Some(var)
            }
            None => {
                let suggestion = self.nearest(&name.text);
                let mut diagnostic =
                    Diagnostic::new("E0208", format!("`'{}'` has not been declared.", name.text))
                        .primary(name.span, "used here")
                        .rule("a name is declared before it is used");
                if let Some(near) = suggestion {
                    diagnostic = diagnostic
                        .tip(format!("a variable named `'{near}'` is declared."))
                        .fix(format!("write `'{near}'`, if that is what was meant."));
                } else {
                    diagnostic = diagnostic.fix("declare it above, with `var`.");
                }
                self.error(diagnostic);
                None
            }
        }
    }

    fn changeable(&mut self, var: &Var, at: Span) -> bool {
        if var.mutable {
            return true;
        }
        self.error(
            Diagnostic::new("E0104", "this cannot be changed, because its declaration never said it could.")
                .secondary(var.declared_at, "declared here, and `mut` is not in the chain")
                .primary(at, "changed here")
                .rule("a variable changes only if its declaration says `mut`")
                .tip("`mut` goes between the visibility and the type.")
                .fix("add `mut` to the declaration."),
        );
        false
    }

    fn usable_type(&mut self, ty: Ty, at: Span) -> bool {
        if ty.implemented() {
            return true;
        }
        self.error(
            Diagnostic::new("E0209", format!("`{}` is designed but not built yet.", ty.word()))
                .primary(at, "asked for here")
                .rule("iteration 1 has the binary floats, the integers, `bool` and `str`")
                .tip("the decimal formats and `er` exist in the design and have no implementation behind them.")
                .fix("use a binary float or an integer for now."),
        );
        false
    }

    // ---- values -----------------------------------------------------------------

    /// Check an expression, against a type when there is one in reach.
    fn expr(&mut self, expr: &AExpr, expected: Option<Ty>) -> Option<ir::Expr> {
        match expr {
            AExpr::Math { inner, .. } => self.expr(inner, expected),

            AExpr::Literal { text, span } => {
                let ty = self.need_type(*span, expected, "a written value")?;
                self.literal(text, ty, *span)
            }

            AExpr::Number { text, span } => {
                let ty = self.need_type(*span, expected, "a number")?;
                self.literal(text, ty, *span)
            }

            AExpr::Name(name) => {
                let var = self.resolve(name)?;
                self.agree(var.ty, expected, name.span)?;
                Some(ir::Expr::Load { slot: var.slot, ty: var.ty, span: name.span })
            }

            AExpr::TimeNow { span } => {
                let ty = self.need_type(*span, expected, "the clock")?;
                if !ty.is_float() {
                    self.error(
                        Diagnostic::new("E0210", format!("the clock cannot be read as `{}`.", ty.word()))
                            .primary(*span, "read here")
                            .rule("`time.now` is a count of seconds, which is a float")
                            .tip("seconds come with a fraction, and a whole number would throw it away.")
                            .fix("read it into a `b64`."),
                    );
                    return None;
                }
                Some(ir::Expr::TimeNow { ty, span: *span })
            }

            AExpr::Percent { inner, span } => {
                let ty = self.need_type(*span, expected, "a percentage")?;
                if !ty.is_float() {
                    self.error(
                        Diagnostic::new("E0211", format!("a percentage cannot be read as `{}`.", ty.word()))
                            .primary(*span, "written here")
                            .rule("a percentage is a fraction of one hundred, which a whole number cannot hold")
                            .tip("`20%` is 0.2, and a whole number would round it to nothing.")
                            .fix("read it into a float type, or a decimal one when those arrive."),
                    );
                    return None;
                }
                let value = self.expr(inner, Some(ty))?;
                let hundred = self.literal("100", ty, *span)?;
                Some(ir::Expr::Binary {
                    op: BinOp::Div,
                    ty,
                    lhs: Box::new(value),
                    rhs: Box::new(hundred),
                    span: *span,
                })
            }

            AExpr::Unary { operand, span, .. } => {
                let value = self.expr(operand, expected)?;
                let ty = value.ty();
                if ty.is_integer() && !ty.is_signed() {
                    self.error(
                        Diagnostic::new("E0212", format!("`{}` has no negative values.", ty.word()))
                            .primary(*span, "negated here")
                            .rule("an unsigned type holds nothing below zero")
                            .fix(format!("use `i{}` instead.", ty.int_bits().unwrap_or(64))),
                    );
                    return None;
                }
                Some(ir::Expr::Neg { ty, operand: Box::new(value), span: *span })
            }

            AExpr::Compare { op, lhs, rhs, span } => {
                // A comparison answers `bool` whatever its two sides are, so what is
                // expecting it says nothing about them.
                self.agree(Ty::Bool, expected, *span)?;
                // Whichever side knows what it is goes first, and tells the other. A
                // comparison says nothing about its sides, so if neither knows, nothing
                // does -- but `-97 < 'x'` is perfectly well typed and reading only the
                // left of it would say otherwise.
                let (lhs, rhs) = self.two_sides(lhs, rhs, None)?;
                let operands = lhs.ty();

                let orderable = operands.is_integer() || operands.is_float();
                if matches!(op, CmpOp::Less | CmpOp::Greater) && !orderable {
                    self.error(
                        Diagnostic::new("E0220", format!("`{}` cannot be put in order.", operands.word()))
                            .primary(*span, format!("compared with `{}` here", op.word()))
                            .rule("`<` and `>` order numbers")
                            .tip("`=` works on anything, since two things of the same type are either the same or not.")
                            .fix("use `=`, or compare numbers."),
                    );
                    return None;
                }

                Some(ir::Expr::Compare {
                    op: *op,
                    operands,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span: *span,
                })
            }

            AExpr::Binary { op, lhs, rhs, span } => {
                // Whichever side knows what it is goes first and tells the other, so both
                // `'x' + 1` and `1 + 'x'` read the 1 as whatever `'x'` is.
                let (lhs, rhs) = self.two_sides(lhs, rhs, expected)?;
                let ty = lhs.ty();
                if !ty.is_integer() && !ty.is_float() {
                    self.error(
                        Diagnostic::new("E0213", format!("`{}` cannot be calculated with.", ty.word()))
                            .primary(*span, "used in arithmetic here")
                            .rule("arithmetic works on numbers")
                            .fix("use a numeric type."),
                    );
                    return None;
                }
                Some(ir::Expr::Binary { op: *op, ty, lhs: Box::new(lhs), rhs: Box::new(rhs), span: *span })
            }
        }
    }

    /// Check two operands, starting from whichever of them carries a type.
    ///
    /// Returns them in the order they were written, whichever order they were checked in.
    fn two_sides(
        &mut self,
        lhs: &AExpr,
        rhs: &AExpr,
        expected: Option<Ty>,
    ) -> Option<(ir::Expr, ir::Expr)> {
        if expected.is_some() || self_typing(lhs) || !self_typing(rhs) {
            let lhs = self.expr(lhs, expected)?;
            let ty = lhs.ty();
            let rhs = self.expr(rhs, Some(ty))?;
            return Some((lhs, rhs));
        }
        let rhs = self.expr(rhs, None)?;
        let lhs = self.expr(lhs, Some(rhs.ty()))?;
        Some((lhs, rhs))
    }

    /// A literal has no type of its own, so refuse politely when nothing supplies one.
    fn need_type(&mut self, at: Span, expected: Option<Ty>, what: &str) -> Option<Ty> {
        match expected {
            Some(ty) => Some(ty),
            None => {
                self.error(
                    Diagnostic::new("E0214", format!("nothing here says how to read {what}."))
                        .primary(at, "written here")
                        .rule("a value takes its type from what is expecting it, and there must be something expecting it")
                        .tip("a declaration's type is what usually says; in a print list there is nothing to say it.")
                        .fix("put the value in a variable first, and print that."),
                );
                None
            }
        }
    }

    fn agree(&mut self, found: Ty, expected: Option<Ty>, at: Span) -> Option<()> {
        match expected {
            Some(wanted) if wanted != found => {
                self.error(
                    Diagnostic::new("E0215", format!("this is `{}` where `{}` was expected.", found.word(), wanted.word()))
                        .primary(at, format!("this is `{}`", found.word()))
                        .rule("nothing converts on its own, not even between two numbers")
                        .tip("Luarust has no widening: a `b32` is not a `b64` with fewer bits, it is a different type.")
                        .fix(format!("declare it as `{}`, or use a `{}` here instead.", wanted.word(), wanted.word())),
                );
                None
            }
            _ => Some(()),
        }
    }

    /// Read a written number as the type that is expecting it.
    fn literal(&mut self, text: &str, ty: Ty, at: Span) -> Option<ir::Expr> {
        if ty == Ty::Str {
            return Some(ir::Expr::Const(Value::text(text)));
        }
        if ty == Ty::Bool {
            return match text {
                "true" => Some(ir::Expr::Const(Value::Bool(true))),
                "false" => Some(ir::Expr::Const(Value::Bool(false))),
                _ => {
                    self.error(
                        Diagnostic::new("E0216", format!("`{text}` is not `true` or `false`."))
                            .primary(at, "written here")
                            .rule("a `bool` is `true` or `false`, and nothing else is either")
                            .tip("Luarust has no truthiness: a number is never a condition.")
                            .fix("write `'true'` or `'false'`."),
                    );
                    None
                }
            };
        }

        if let Some(fmt) = format_of(ty) {
            return match binary::from_decimal::<8>(fmt, Round::TiesToEven, text) {
                Ok(bits) => Some(ir::Expr::Const(Value::float(ty, bits))),
                Err(why) => {
                    self.error(self.bad_number(text, ty, at, why));
                    None
                }
            };
        }

        // The integers.
        let trimmed = text.trim();
        let parsed = trimmed.parse::<i128>();
        match parsed {
            Ok(number) => {
                let width = ty.int_bits().expect("an integer type has a width");
                let fits = if ty.is_signed() {
                    let limit = 1i128 << (width - 1);
                    (-limit..limit).contains(&number)
                } else {
                    (0..(1i128 << width)).contains(&number)
                };
                if !fits {
                    let (low, high) = int_range(ty);
                    self.error(
                        Diagnostic::new("E0217", format!("`{text}` is out of range for `{}`.", ty.word()))
                            .primary(at, "written here")
                            .rule("a written value must be a value of the type it is read as")
                            .tip(format!("`{}` holds {low} to {high}.", ty.word()))
                            .fix("use a wider type, or a smaller number."),
                    );
                    return None;
                }
                let bits = if width == 64 {
                    number as u64
                } else {
                    (number as u64) & ((1u64 << width) - 1)
                };
                Some(ir::Expr::Const(Value::Num { ty, bits }))
            }
            Err(_) => {
                self.error(
                    Diagnostic::new("E0218", format!("`{text}` is not a whole number."))
                        .primary(at, "written here")
                        .rule("a written value must be a value of the type it is read as")
                        .tip(format!("`{}` holds whole numbers, so it has nowhere to put a fraction.", ty.word()))
                        .fix("remove the fraction, or read it as a float type such as `b64`."),
                );
                None
            }
        }
    }

    fn bad_number(&self, text: &str, ty: Ty, at: Span, why: binary::Invalid) -> Diagnostic {
        let (message, tip) = match why {
            binary::Invalid::NoDigits => ("there are no digits in this.".to_string(), "a number needs at least one digit."),
            binary::Invalid::TwoPoints => (format!("`{text}` has two decimal points."), "a number has at most one."),
            binary::Invalid::Unexpected(c) => (format!("`{c}` is not part of a number."), "a number is digits, an optional sign, and at most one decimal point."),
            binary::Invalid::TooLong => (format!("`{text}` has more digits than can be read exactly."), "the digits are kept exactly while they are rounded, and there is a limit to how many fit."),
        };
        Diagnostic::new("E0219", message)
            .primary(at, format!("read as `{}` here", ty.word()))
            .rule("a written value must be a value of the type it is read as")
            .tip(tip)
            .fix("write a plain number, such as `'1000'` or `'0.1'`.")
    }
}

/// Whether an expression settles its own type without being told.
///
/// A variable does, because it was declared. A written number does not, and neither does
/// the clock or a percentage: those take the type of whatever is expecting them.
fn self_typing(expr: &AExpr) -> bool {
    match expr {
        AExpr::Name(_) => true,
        AExpr::Compare { .. } => true,
        AExpr::Math { inner, .. } => self_typing(inner),
        AExpr::Unary { operand, .. } => self_typing(operand),
        AExpr::Binary { lhs, rhs, .. } => self_typing(lhs) || self_typing(rhs),
        AExpr::Literal { .. } | AExpr::Number { .. } | AExpr::TimeNow { .. } | AExpr::Percent { .. } => false,
    }
}

fn int_range(ty: Ty) -> (i128, i128) {
    let width = ty.int_bits().unwrap_or(64);
    if ty.is_signed() {
        (-(1i128 << (width - 1)), (1i128 << (width - 1)) - 1)
    } else {
        (0, (1i128 << width) - 1)
    }
}

/// How many single-character edits separate two names, capped so that a long name does
/// not cost a long walk.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().take(64).collect();
    let b: Vec<char> = b.chars().take(64).collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j] + cost).min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> (Checked, Vec<Diagnostic>) {
        let lexed = luarust_lex::lex(source);
        assert!(lexed.ok(), "lexing failed: {:#?}", lexed.errors);
        let parsed = luarust_parse::parse(source, &lexed.tokens);
        assert!(parsed.ok(), "parsing failed: {:#?}", parsed.errors);
        check(&parsed.program)
    }

    fn clean(source: &str) -> Checked {
        let (checked, errors) = run(source);
        assert!(errors.is_empty(), "expected no errors, got {errors:#?}");
        checked
    }

    fn codes(source: &str) -> Vec<String> {
        run(source).1.into_iter().map(|e| e.code).collect()
    }

    #[test]
    fn the_readme_programs_all_check() {
        clean("loop.temp.range.ui8 ['i'] = ['1', '5'] {\n print['i' \\n];\n}\n");
        clean(
            "var.local.mut.ui32 ['total'] = ['0'];\n\
             loop.temp.range.ui32 ['i'] = ['1', '10'] { handback 'i' as 'total'; }\n\
             print[\"total is \" 'total' \\n];\n",
        );
        clean("var.local.b16 ['a'] = ['0.1'];\nprint['a' \\n];\n");
        clean(
            "var.local.mut.ui64 ['sum'] = ['0'];\n\
             var.local.b64 ['start'] = [time.now];\n\
             loop.temp.range.ui64 ['i'] = ['1', '100'] {\n\
                 set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];\n\
             }\n\
             var.local.b64 ['elapsed'] = [math { time.now - 'start' }];\n\
             print['sum' \" in \" 'elapsed' \" seconds\\n\"];\n",
        );
    }

    #[test]
    fn a_name_that_was_never_declared_is_reported_with_a_guess() {
        let (_, errors) = run("var.local.b16 ['count'] = ['1'];\nprint['cont'];");
        assert_eq!(errors[0].code, "E0208");
        assert!(errors[0].tips[0].contains("'count'"), "{:?}", errors[0].tips);
    }

    #[test]
    fn a_restricted_variable_cannot_be_touched() {
        // Said by saying nothing, which is the default.
        let (_, errors) = run("var.b16 ['x'] = ['1'];\nprint['x'];");
        assert_eq!(errors[0].code, "E0207");
        // And said out loud.
        assert_eq!(codes("var.restricted.b16 ['x'] = ['1'];\nprint['x'];"), ["E0207"]);
        // The declaration itself is fine either way.
        clean("var.b16 ['x'] = ['1'];");
    }

    #[test]
    fn changing_something_that_never_said_mut_is_reported() {
        assert_eq!(
            codes("var.local.ui32 ['total'] = ['0'];\nset ['total'] = ['55'];"),
            ["E0104"]
        );
        clean("var.local.mut.ui32 ['total'] = ['0'];\nset ['total'] = ['55'];");
    }

    #[test]
    fn the_two_lists_have_to_be_the_same_length() {
        let (_, errors) = run("var.local.b16 ['a', 'b', 'c'] = ['1', '2'];");
        assert_eq!(errors[0].code, "E0206");
        assert_eq!(errors[0].labels.len(), 2, "both lists are pointed at");
        assert!(errors[0].message.contains("3 names here and 2 values"));
    }

    #[test]
    fn nothing_converts_on_its_own() {
        let (_, errors) = run(
            "var.local.b32 ['a'] = ['1'];\n\
             var.local.b64 ['b'] = [math { 'a' + 1 }];",
        );
        assert_eq!(errors[0].code, "E0215");
        assert!(errors[0].message.contains("`b32` where `b64` was expected"));
    }

    #[test]
    fn a_value_that_will_not_fit_its_type_is_reported() {
        let (_, errors) = run("var.local.ui8 ['small'] = ['300'];");
        assert_eq!(errors[0].code, "E0217");
        assert!(errors[0].tips[0].contains("0 to 255"), "{:?}", errors[0].tips);

        // A fraction has nowhere to go in a whole number.
        assert_eq!(codes("var.local.i32 ['x'] = ['1.5'];"), ["E0218"]);
        // But a float takes it, and one too large becomes an infinity rather than an error.
        clean("var.local.b16 ['x'] = ['70000'];");
    }

    #[test]
    fn the_types_that_are_not_built_yet_say_so() {
        assert_eq!(codes("var.local.d64 ['money'] = ['19.99'];"), ["E0209"]);
        assert_eq!(codes("var.local.er ['third'] = ['1'];"), ["E0209"]);
    }

    #[test]
    fn the_visibility_default_can_be_made_an_error() {
        clean("var.b16 ['x'] = ['1'];");
        let (_, errors) = run("defaults.no-visibility-stated.error;\nvar.b16 ['x'] = ['1'];");
        assert_eq!(errors[0].code, "E0202");
        // And it applies to declarations written above it, too.
        assert_eq!(
            codes("var.b16 ['x'] = ['1'];\ndefaults.no-visibility-stated.error;"),
            ["E0202"]
        );
    }

    #[test]
    fn a_default_that_is_not_a_setting_is_reported() {
        assert_eq!(codes("defaults.wobble.error;"), ["E0201"]);
        assert_eq!(codes("defaults.overflow.sideways;"), ["E0200"]);
        clean("defaults.overflow.trap;");
    }

    #[test]
    fn overflow_is_wrapping_unless_a_default_says_otherwise() {
        assert_eq!(clean("var.local.b16 ['x'] = ['1'];").overflow, Overflow::Wrap);
        assert_eq!(clean("defaults.overflow.trap;").overflow, Overflow::Trap);
    }

    #[test]
    fn a_printed_value_needs_a_type_from_somewhere() {
        // A bare literal in a print list has nothing to say how to read it.
        let (_, errors) = run("print[math { 1 + 2 }];");
        assert_eq!(errors[0].code, "E0214");
        // A variable brings its own type, so this is fine.
        clean("var.local.b16 ['x'] = ['1'];\nprint[math { 'x' + 1 }];");
    }

    #[test]
    fn a_temp_counter_is_gone_afterwards_and_a_perm_one_is_not() {
        assert_eq!(
            codes("loop.temp.range.ui8 ['i'] = ['1','5'] { }\nprint['i'];"),
            ["E0208"]
        );
        clean("loop.perm.range.ui8 ['i'] = ['1','5'] { }\nprint['i'];");
    }

    #[test]
    fn the_clock_is_a_float_and_a_percentage_is_too() {
        clean("var.local.b64 ['t'] = [time.now];");
        assert_eq!(codes("var.local.ui64 ['t'] = [time.now];"), ["E0210"]);
        clean("var.local.b64 ['fifth'] = [math { 20% }];");
        assert_eq!(codes("var.local.ui8 ['fifth'] = [math { 20% }];"), ["E0211"]);
    }

    #[test]
    fn an_unsigned_number_cannot_be_negated() {
        assert_eq!(codes("var.local.ui8 ['x'] = [math { -1 }];"), ["E0212"]);
        clean("var.local.i8 ['x'] = [math { -1 }];");
    }

    #[test]
    fn declaring_the_same_name_twice_in_one_block_is_reported() {
        let (_, errors) = run("var.local.b16 ['x'] = ['1'];\nvar.local.b16 ['x'] = ['2'];");
        assert_eq!(errors[0].code, "E0203");
        // A block of its own is a different matter.
        clean("var.local.b16 ['x'] = ['1'];\nloop.temp.range.ui8 ['i'] = ['1','2'] { var.local.b16 ['x'] = ['2']; }");
    }

    #[test]
    fn handback_needs_both_sides_to_agree() {
        let (_, errors) = run(
            "var.local.mut.ui32 ['total'] = ['0'];\n\
             var.local.b16 ['bit'] = ['1'];\n\
             handback 'bit' as 'total';",
        );
        assert_eq!(errors[0].code, "E0204");
    }

    #[test]
    fn every_name_becomes_a_slot_and_every_literal_a_value() {
        let checked = clean(
            "var.local.mut.ui32 ['total'] = ['0'];\n\
             loop.temp.range.ui32 ['i'] = ['1', '10'] { handback 'i' as 'total'; }\n",
        );
        assert_eq!(checked.slots, 2, "total and i");
        let ir::Stmt::Store { slot, value, .. } = &checked.stmts[0] else { panic!("not a store") };
        assert_eq!(*slot, 0);
        assert!(matches!(value, ir::Expr::Const(Value::Num { ty: Ty::U32, bits: 0 })));
        assert!(matches!(&checked.stmts[1], ir::Stmt::Loop { ty: Ty::U32, .. }));
    }

    #[test]
    fn every_mistake_in_a_file_is_found_at_once() {
        let (_, errors) = run(
            "var.local.ui8 ['small'] = ['300'];\n\
             var.local.b16 ['x'] = ['1'];\n\
             set ['x'] = ['2'];\n\
             print['nope'];\n",
        );
        assert_eq!(
            errors.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            ["E0217", "E0104", "E0208"]
        );
    }
}

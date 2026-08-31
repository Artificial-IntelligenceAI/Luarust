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

// `value` moved down to `luarust-core`, because a program needs it while it runs and
// needs the checker only before it does. It is still named from here.
pub use luarust_core::value;

use crate::value::{Engine, Floats};
use ir::Checked;
use luarust_core::heap::Collect;
use luarust_diag::{Diagnostic, Span};
use luarust_num::binary::{self, Round};
use luarust_parse::ast::{
    self, BinOp, Expr as AExpr, Lifetime, PrintItem, Stmt as AStmt, Ty, Visibility,
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

/// What a project has already decided, before a file says anything about itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Start {
    pub overflow: Overflow,
    pub visibility_required: bool,
    pub collect: Collect,
    pub floats: Floats,
    pub engine: Engine,
}

impl Default for Start {
    fn default() -> Self {
        Start {
            overflow: Overflow::Wrap,
            visibility_required: false,
            collect: Collect::Off,
            floats: Floats::Exact,
            engine: Engine::Vm,
        }
    }
}

/// Check a parsed program, with nothing decided beforehand.
pub fn check(program: &ast::Program) -> (Checked, Vec<Diagnostic>) {
    check_with(program, Start::default())
}

/// Check a parsed program that a project already had settings for.
///
/// A `defaults.` line in the file still wins, because whatever a file says about itself
/// is the last word on it.
pub fn check_with(program: &ast::Program, start: Start) -> (Checked, Vec<Diagnostic>) {
    let mut checker = Checker {
        scopes: vec![HashMap::new()],
        slots: 0,
        overflow: start.overflow,
        collect: start.collect,
        floats: start.floats,
        engine: start.engine,
        visibility_required: start.visibility_required,
        signatures: HashMap::new(),
        funcs: Vec::new(),
        inside: None,
        loop_counter: None,
        errors: Vec::new(),
    };

    // Defaults are read first, wherever in the file they were written, so that a
    // declaration above one still answers to it.
    for stmt in &program.stmts {
        if let AStmt::Defaults(defaults) = stmt {
            checker.apply_default(defaults);
        }
    }

    // Every signature is read before any body is, so a function may be called above the
    // line it is written on, and two functions may call each other. A function is not a
    // statement -- it does not happen at a point in the program -- so nothing about it
    // depends on where in the file it sits. Variables are the opposite, and stay so.
    checker.collect_signatures(&program.stmts);
    checker.check_bodies(&program.stmts);

    let stmts = checker.block(&program.stmts);
    let checked = Checked {
        stmts,
        funcs: std::mem::take(&mut checker.funcs),
        slots: checker.slots,
        overflow: checker.overflow,
        collect: checker.collect,
        floats: checker.floats,
        engine: checker.engine,
    };
    (checked, checker.errors)
}

struct Checker {
    scopes: Vec<HashMap<String, Var>>,
    slots: usize,
    overflow: Overflow,
    collect: Collect,
    floats: Floats,
    engine: Engine,
    visibility_required: bool,
    /// Every function's name, and where it sits in `funcs`.
    signatures: HashMap<String, Signature>,
    funcs: Vec<ir::Function>,
    /// What the function being checked answers, when one is being checked. The outer
    /// `Option` says whether we are in a function at all; the inner, whether it answers.
    inside: Option<Option<Ty>>,
    /// The innermost loop's counter, when there is a loop. The outer `Option` says
    /// whether we are in one at all; the inner, whether it counts.
    loop_counter: Option<Option<(usize, Ty)>>,
    errors: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
struct Signature {
    index: usize,
    params: Vec<Ty>,
    returns: Option<Ty>,
    declared_at: Span,
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
                AStmt::If(if_stmt) => out.push(self.if_stmt(if_stmt)),
                AStmt::While(while_stmt) => out.push(self.while_stmt(while_stmt)),
                AStmt::Break(break_stmt) => {
                    if let Some(checked) = self.break_stmt(break_stmt) {
                        out.push(checked);
                    }
                }
                // Read in their own passes, before and after this one.
                AStmt::Func(_) => {}
                AStmt::Return(ret) => {
                    if let Some(checked) = self.return_stmt(ret) {
                        out.push(checked);
                    }
                }
                AStmt::Call(call) => {
                    if let Some(checked) = self.call_stmt(call) {
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
            let Some(var) = self.resolve(target.name()) else { continue };
            if !self.changeable(&var, target.span()) {
                continue;
            }
            match target {
                ast::Target::Name(_) => {
                    let Some(value) = self.expr(value, Some(var.ty)) else { continue };
                    out.push(ir::Stmt::Store { slot: var.slot, value, span: target.span() });
                }
                // Writing one element rather than the whole array. The array itself is
                // not being changed -- it is the same array afterwards -- so what `mut`
                // has to say about it is the same either way.
                ast::Target::Element { at, span, .. } => {
                    let Some(array) = self.expr(
                        &AExpr::Name(target.name().clone()),
                        Some(var.ty),
                    ) else {
                        continue;
                    };
                    let Some(of) = var.ty.array() else {
                        self.error(
                            Diagnostic::new("E0138", format!(
                                "`{}` is not an array, so it has no elements.",
                                var.ty.written()
                            ))
                            .primary(*span, "indexed here")
                            .rule("only an array is indexed")
                            .fix("change the whole variable instead."),
                        );
                        continue;
                    };
                    let wanted = of.dims().len().max(1);
                    if at.len() != wanted {
                        self.error(
                            Diagnostic::new("E0139", format!(
                                "`{}` takes {} to index it, and {} given here.",
                                var.ty.written(),
                                count_of(wanted, "index").replace("indexs", "indices"),
                                at.len()
                            ))
                            .primary(*span, "indexed here")
                            .rule("an array takes one index for each dimension it has")
                            .fix(format!("give {wanted} of them.")),
                        );
                        continue;
                    }
                    let mut indices = Vec::with_capacity(at.len());
                    let mut good = true;
                    for index in at {
                        match self.expr(index, Some(Ty::U32)) {
                            Some(index) if index.ty().is_integer() => indices.push(index),
                            _ => good = false,
                        }
                    }
                    if !good {
                        continue;
                    }
                    let Some(value) = self.expr(value, Some(of.element)) else { continue };
                    out.push(ir::Stmt::StoreAt { array, at: indices, value, span: *span });
                }
            }
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
        if !loop_stmt.ty.is_number() {
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
        let outer = self.loop_counter;
        self.loop_counter = Some(Some((slot, loop_stmt.ty)));
        let body = self.block(&loop_stmt.body);
        self.loop_counter = outer;
        self.scopes.pop();

        Some(ir::Stmt::Loop { slot, ty: loop_stmt.ty, from, to, body, span: loop_stmt.span })
    }

    /// `if`, its `else-if`s and its `else`.
    ///
    /// Each body is its own scope, so a variable declared inside an arm is gone at the
    /// closing brace. Unlike a loop there is nothing to say about that: an `if` declares
    /// nothing of its own, so there is no counter whose life anybody has to decide.
    fn if_stmt(&mut self, if_stmt: &ast::If) -> ir::Stmt {
        let mut arms = Vec::new();
        for arm in &if_stmt.arms {
            let condition = self.condition(&arm.condition);
            self.scopes.push(HashMap::new());
            let body = self.block(&arm.body);
            self.scopes.pop();
            if let Some(condition) = condition {
                arms.push(ir::Arm { condition, body });
            }
        }

        let otherwise = match &if_stmt.otherwise {
            Some(body) => {
                self.scopes.push(HashMap::new());
                let checked = self.block(body);
                self.scopes.pop();
                checked
            }
            None => Vec::new(),
        };

        ir::Stmt::If { arms, otherwise, span: if_stmt.span }
    }

    // ---- functions ---------------------------------------------------------------

    /// Read every function's header, without opening a single body.
    fn collect_signatures(&mut self, stmts: &[AStmt]) {
        for stmt in stmts {
            let AStmt::Func(func) = stmt else { continue };

            if let Some(already) = self.signatures.get(&func.name.text) {
                let first = already.declared_at;
                self.error(
                    Diagnostic::new("E0123", format!("`{}` is declared twice.", func.name.text))
                        .secondary(first, "first declared here")
                        .primary(func.name.span, "and again here")
                        .rule("a name means one thing in the place it is written")
                        .fix("rename one of them."),
                );
                continue;
            }

            let index = self.funcs.len();
            self.signatures.insert(
                func.name.text.clone(),
                Signature {
                    index,
                    params: func.params.iter().map(|p| p.ty).collect(),
                    returns: func.returns,
                    declared_at: func.name.span,
                },
            );
            // A placeholder, so the index is stable while the bodies are still unchecked
            // and one function can already be seen calling another.
            self.funcs.push(ir::Function {
                name: func.name.text.clone(),
                params: func.params.iter().map(|p| p.ty).collect(),
                returns: func.returns,
                slots: 0,
                body: Vec::new(),
                span: func.span,
            });
        }
    }

    /// Check every body, now that every signature is known.
    fn check_bodies(&mut self, stmts: &[AStmt]) {
        for stmt in stmts {
            let AStmt::Func(func) = stmt else { continue };
            let Some(signature) = self.signatures.get(&func.name.text).cloned() else { continue };

            // A body has its own slots and its own scope. It cannot see the variables
            // outside it -- only its parameters and whatever it declares.
            let outer_slots = std::mem::replace(&mut self.slots, 0);
            let outer_scopes = std::mem::replace(&mut self.scopes, vec![HashMap::new()]);
            let outer_inside = self.inside.replace(func.returns);
            // A function is not inside whatever loop its declaration happens to sit in.
            let outer_loop = self.loop_counter.take();

            for param in &func.params {
                let slot = self.next_slot();
                self.declare(
                    &param.name.text,
                    Var {
                        slot,
                        ty: param.ty,
                        mutable: false,
                        visibility: Visibility::Local,
                        declared_at: param.name.span,
                        visibility_at: None,
                    },
                );
            }

            let body = self.block(&func.body);

            // A function that answers something has to answer on every path out of it.
            if let Some(answers) = func.returns.filter(|_| !always_returns(&body)) {
                self.error(
                    Diagnostic::new("E0124", format!("`{}` does not answer on every path.", func.name.text))
                        .primary(func.returns_span, format!("it says it answers `{}`", answers.word()))
                        .rule("a function that states an answer gives one however it ends")
                        .tip("an `if` without an `else` has a path where nothing was returned.")
                        .fix("add a `return` at the end, or an `else` that has one."),
                );
            }

            self.funcs[signature.index].slots = self.slots;
            self.funcs[signature.index].body = body;

            self.slots = outer_slots;
            self.scopes = outer_scopes;
            self.inside = outer_inside;
            self.loop_counter = outer_loop;
        }
    }

    fn return_stmt(&mut self, ret: &ast::Return) -> Option<ir::Stmt> {
        let Some(expected) = self.inside else {
            self.error(
                Diagnostic::new("E0125", "there is nothing here to return from.".to_string())
                    .primary(ret.span, "written here")
                    .rule("`return` leaves the function it is written in")
                    .tip("at the top level a program simply reaches its end.")
                    .fix("move it inside a function."),
            );
            return None;
        };

        match (expected, &ret.value) {
            (None, None) => Some(ir::Stmt::Return { value: None, span: ret.span }),
            (None, Some(value)) => {
                self.error(
                    Diagnostic::new("E0126", "this returns a value from a function that answers nothing.".to_string())
                        .primary(value.span(), "this value")
                        .rule("a function answers what its chain says it answers")
                        .fix("write `return;`, or give the function a type instead of `nothing`."),
                );
                None
            }
            (Some(ty), None) => {
                self.error(
                    Diagnostic::new("E0127", format!("this returns nothing from a function that answers `{}`.", ty.word()))
                        .primary(ret.span, "written here")
                        .rule("a function answers what its chain says it answers")
                        .fix(format!("return a `{}`.", ty.word())),
                );
                None
            }
            (Some(ty), Some(value)) => {
                let checked = self.expr(value, Some(ty))?;
                Some(ir::Stmt::Return { value: Some(checked), span: ret.span })
            }
        }
    }

    /// `greet['Tankun'];` — a call written for what it does. Whatever it answers, if
    /// anything, is dropped, which is the only place that is allowed.
    fn call_stmt(&mut self, expr: &AExpr) -> Option<ir::Stmt> {
        let AExpr::Call { name, args, span } = expr else {
            unreachable!("a call statement holds a call")
        };
        let (index, _, args) = self.resolve_call(name, args, *span)?;
        Some(ir::Stmt::Call { func: index, args, span: *span })
    }

    /// The part every call has in common: find it, count the arguments, check each one.
    fn resolve_call(
        &mut self,
        name: &ast::Ident,
        args: &[AExpr],
        span: Span,
    ) -> Option<(usize, Option<Ty>, Vec<ir::Expr>)> {
        let Some(signature) = self.signatures.get(&name.text).cloned() else {
            let guess = self
                .signatures
                .keys()
                .filter(|known| edit_distance(known, &name.text) <= 2.max(name.text.chars().count() / 3))
                .min_by_key(|known| edit_distance(known, &name.text))
                .cloned();
            let mut diagnostic =
                Diagnostic::new("E0128", format!("there is no function called `{}`.", name.text))
                    .primary(name.span, "called here")
                    .rule("a call names a function the program declares");
            // A near-miss on a statement keyword is the likelier mistake, and the list of
            // functions would never have suggested it.
            let keyword = ["print", "var", "set", "handback", "loop", "if", "fn", "return"]
                .into_iter()
                .filter(|word| edit_distance(word, &name.text) <= 2)
                .min_by_key(|word| edit_distance(word, &name.text));
            if let Some(word) = keyword {
                diagnostic = diagnostic.fix(format!("did you mean `{word}`?"));
            } else if let Some(guess) = guess {
                diagnostic = diagnostic.fix(format!("did you mean `{guess}`?"));
            } else {
                diagnostic = diagnostic
                    .tip("a variable is written in quotes; a bare word before `[` is a function.")
                    .fix("check the spelling, or declare it.");
            }
            self.error(diagnostic);
            return None;
        };

        if args.len() != signature.params.len() {
            self.error(
                Diagnostic::new("E0130", format!(
                    "`{}` takes {}, and {} given here.",
                    name.text,
                    count_of(signature.params.len(), "parameter"),
                    if args.len() == 1 { "1 is".to_string() } else { format!("{} are", args.len()) },
                ))
                .primary(span, "called here")
                .secondary(signature.declared_at, "declared here")
                .rule("a call gives one argument for each parameter")
                .fix("add or remove arguments until the counts agree."),
            );
            return None;
        }

        let mut checked = Vec::with_capacity(args.len());
        for (arg, ty) in args.iter().zip(&signature.params) {
            checked.push(self.expr(arg, Some(*ty))?);
        }

        Some((signature.index, signature.returns, checked))
    }

    /// The functions the language ships with, which no program declares.
    fn built_in(
        &mut self,
        name: &ast::Ident,
        args: &[AExpr],
        span: Span,
        expected: Option<Ty>,
    ) -> Option<Option<ir::Expr>> {
        match name.text.as_str() {
            "count" => Some(self.count_of(args, span, expected)),
            "filled" => Some(self.filled(args, span, expected)),
            _ => None,
        }
    }

    /// `count['xs']` — how many elements, as whatever type is expecting the answer.
    fn count_of(&mut self, args: &[AExpr], span: Span, expected: Option<Ty>) -> Option<ir::Expr> {
        let [array] = args else {
            self.error(
                Diagnostic::new("E0130", format!("`count` takes 1 array, and {} given here.", args.len()))
                    .primary(span, "called here")
                    .rule("a call gives one argument for each parameter")
                    .fix("give it one array."),
            );
            return None;
        };
        let array = self.expr(array, None)?;
        if array.ty().array().is_none() {
            self.error(
                Diagnostic::new("E0138", format!("`{}` is not an array, so it has no count.", array.ty().written()))
                    .primary(span, "counted here")
                    .rule("only an array has a number of elements")
                    .fix("count an array instead."),
            );
            return None;
        }
        // Whatever is expecting it, so `loop … = [|1|, count['xs']]` counts in the
        // counter's own type rather than forcing a conversion the language does not have.
        let ty = expected.unwrap_or(Ty::U32);
        if !ty.is_integer() {
            self.error(
                Diagnostic::new("E0141", format!("a count cannot be read as `{}`.", ty.written()))
                    .primary(span, "read here")
                    .rule("a count is a whole number")
                    .fix("read it into a whole number."),
            );
            return None;
        }
        Some(ir::Expr::Count { array: Box::new(array), ty, span })
    }

    /// `filled[|10|, |0|]` — an array of that many of that.
    fn filled(&mut self, args: &[AExpr], span: Span, expected: Option<Ty>) -> Option<ir::Expr> {
        let ty = self.need_type(span, expected, "an array")?;
        let Some(of) = ty.array() else {
            self.error(
                Diagnostic::new("E0136", format!("`filled` makes an array, and `{}` is not one.", ty.written()))
                    .primary(span, "written here")
                    .rule("a list of values makes an array, and only an array")
                    .fix("declare it as an array."),
            );
            return None;
        };

        // A fixed array already knows how many, so it is given only what to fill with.
        let (length, value) = match (of.length(), args) {
            (Some(fixed), [value]) => {
                let length = ir::Expr::Const(Value::Num { ty: Ty::U32, bits: fixed as u64 });
                (length, value)
            }
            (None, [length, value]) => (self.expr(length, Some(Ty::U32))?, value),
            (Some(_), _) => {
                self.error(
                    Diagnostic::new("E0130", format!(
                        "`filled` takes 1 value for a `{}`, and {} given here.",
                        ty.written(),
                        args.len()
                    ))
                    .primary(span, "called here")
                    .rule("a fixed array already knows how many it holds")
                    .fix("give it only what to fill with."),
                );
                return None;
            }
            (None, _) => {
                self.error(
                    Diagnostic::new("E0130", format!(
                        "`filled` takes a length and a value for a `{}`, and {} given here.",
                        ty.written(),
                        args.len()
                    ))
                    .primary(span, "called here")
                    .rule("a growable array is told how many to make")
                    .fix("give it a length and a value."),
                );
                return None;
            }
        };

        let value = self.expr(value, Some(of.element))?;
        Some(ir::Expr::Filled {
            ty,
            length: Box::new(length),
            value: Box::new(value),
            span,
        })
    }

    /// `name[a, b]` where a value is wanted, so it has to answer one.
    fn call(
        &mut self,
        name: &ast::Ident,
        args: &[AExpr],
        span: Span,
        expected: Option<Ty>,
    ) -> Option<ir::Expr> {
        if let Some(made) = self.built_in(name, args, span, expected) {
            return made;
        }
        let declared_at = self.signatures.get(&name.text).map(|s| s.declared_at);
        let (index, returns, args) = self.resolve_call(name, args, span)?;

        let Some(returns) = returns else {
            let mut diagnostic =
                Diagnostic::new("E0129", format!("`{}` answers nothing, so it has no value here.", name.text))
                    .primary(span, "used as a value here")
                    .rule("a value comes from something that answers one")
                    .fix("call it on its own line, or give it a type to answer.");
            if let Some(at) = declared_at {
                diagnostic = diagnostic.secondary(at, "declared to answer `nothing`");
            }
            self.error(diagnostic);
            return None;
        };

        self.agree(returns, expected, span)?;
        Some(ir::Expr::Call { func: index, ty: returns, args, span })
    }

    /// `loop.while [ … ] { … }`, with a counter of its passes when it asked for one.
    fn while_stmt(&mut self, while_stmt: &ast::While) -> ir::Stmt {
        let condition = self.condition(&while_stmt.condition);

        let counter = while_stmt.counter.as_ref().map(|counter| {
            let slot = self.next_slot();
            let var = Var {
                slot,
                ty: counter.ty,
                mutable: false,
                visibility: Visibility::Local,
                declared_at: counter.name.span,
                visibility_at: Some(counter.lifetime_span),
            };
            if counter.lifetime == Lifetime::Perm {
                self.declare(&counter.name.text, var.clone());
            }
            (counter, slot, var)
        });

        self.scopes.push(HashMap::new());
        if let Some((counter, _, var)) = &counter
            && counter.lifetime == Lifetime::Temp
        {
            self.declare(&counter.name.text, var.clone());
        }

        let outer = self.loop_counter;
        self.loop_counter = Some(counter.as_ref().map(|(c, slot, _)| (*slot, c.ty)));
        let body = self.block(&while_stmt.body);
        self.loop_counter = outer;
        self.scopes.pop();

        ir::Stmt::While {
            counter: counter.map(|(c, slot, _)| (slot, c.ty)),
            condition: condition.unwrap_or(ir::Expr::Const(Value::Bool(false))),
            body,
            span: while_stmt.span,
        }
    }

    /// `break;`, or `break when reached x;` which is an `if` around a `break`.
    fn break_stmt(&mut self, break_stmt: &ast::Break) -> Option<ir::Stmt> {
        let Some(counter) = self.loop_counter else {
            self.error(
                Diagnostic::new("E0133", "there is no loop here to leave.".to_string())
                    .primary(break_stmt.span, "written here")
                    .rule("`break` leaves the loop it is written in")
                    .fix("move it inside a loop."),
            );
            return None;
        };

        let Some(reached) = &break_stmt.reached else {
            return Some(ir::Stmt::Break { span: break_stmt.span });
        };

        let Some((slot, ty)) = counter else {
            self.error(
                Diagnostic::new("E0134", "this loop has no counter to have reached anything.".to_string())
                    .primary(break_stmt.span, "written here")
                    .rule("`break when reached` compares the counter of the loop it is in")
                    .tip("a `loop.while` counts its passes only if it is given a type and a name.")
                    .fix("write plain `break;` inside an `if`, or give the loop a counter."),
            );
            return None;
        };

        let value = self.expr(reached, Some(ty))?;
        Some(ir::Stmt::If {
            arms: vec![ir::Arm {
                condition: ir::Expr::Compare {
                    op: luarust_parse::ast::CmpOp::Equal,
                    operands: ty,
                    lhs: Box::new(ir::Expr::Load { slot, ty, span: break_stmt.span }),
                    rhs: Box::new(value),
                    span: break_stmt.span,
                },
                body: vec![ir::Stmt::Break { span: break_stmt.span }],
            }],
            otherwise: Vec::new(),
            span: break_stmt.span,
        })
    }

    /// Something asked as a question, which has to be a `bool` and nothing else.
    fn condition(&mut self, expr: &AExpr) -> Option<ir::Expr> {
        // Something that settles its own type is asked what it is, so that a wrong answer
        // can be called what it is rather than reported as a conversion nobody wanted.
        // Something that does not -- a written `'true'` -- is told to be a `bool`, since
        // in a condition nothing else would ever tell it.
        let expected = if self_typing(expr) { None } else { Some(Ty::Bool) };
        let checked = self.expr(expr, expected)?;
        if checked.ty() != Ty::Bool {
            self.error(
                Diagnostic::new("E0221", format!("this asks a `{}`, which is not a question.", checked.ty().word()))
                    .primary(expr.span(), "used as a condition here")
                    .rule("a condition is a `bool`")
                    .tip("a comparison answers `bool`, and so does `and`, `or` and `not`.")
                    .fix("compare it against something, as in `math { 'n' > i32 '0' }`."),
            );
            return None;
        }
        Some(checked)
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


    // ---- values -----------------------------------------------------------------

    /// Check an expression, against a type when there is one in reach.
    fn expr(&mut self, expr: &AExpr, expected: Option<Ty>) -> Option<ir::Expr> {
        match expr {
            AExpr::Math { inner, .. } => self.expr(inner, expected),

            AExpr::Literal { text, span } => {
                let ty = self.need_type(*span, expected, "a written value")?;
                self.literal(text, ty, *span)
            }

            // A literal that says what it is, for the places where nothing else does --
            // a comparison, which tells its two sides nothing, being the main one.
            AExpr::TypedLiteral { ty, text, span } => {
                self.agree(*ty, expected, *span)?;
                self.literal(text, *ty, *span)
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
                if !ty.is_float() && ty != Ty::Er {
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

                let orderable = operands.is_number();
                if op.orders() && !orderable {
                    self.error(
                        Diagnostic::new("E0220", format!("`{}` cannot be put in order.", operands.word()))
                            .primary(*span, format!("compared with `{}` here", op.word()))
                            .rule("`<`, `>`, `</=` and `>/=` put numbers in order")
                            .tip("`=` and `!=` work on anything, since two things of the same type are either the same or not.")
                            .fix("use `=` or `!=`, or compare numbers."),
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

            // Both sides of `and`/`or` are questions and so is the answer, which makes
            // this the one place in the language where nothing has to be inferred.
            AExpr::Logic { op, lhs, rhs, span } => {
                self.agree(Ty::Bool, expected, *span)?;
                let lhs = self.condition(lhs)?;
                let rhs = self.condition(rhs)?;
                Some(ir::Expr::Logic {
                    op: *op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span: *span,
                })
            }

            AExpr::Not { operand, span } => {
                self.agree(Ty::Bool, expected, *span)?;
                let operand = self.condition(operand)?;
                Some(ir::Expr::Not { operand: Box::new(operand), span: *span })
            }

            AExpr::Call { name, args, span } => self.call(name, args, *span, expected),

            // A list where a value is wanted is a new array of these. What kind of array
            // has to come from what is expecting it: a list of numbers says nothing about
            // whether they are `ui8`s or `b64`s, and this language never guesses.
            AExpr::Items { items, span } => {
                let ty = self.need_type(*span, expected, "an array")?;
                let Some(of) = ty.array() else {
                    self.error(
                        Diagnostic::new("E0136", format!("this is a list, and `{}` is not an array.", ty.written()))
                            .primary(*span, "written here")
                            .rule("a list of values makes an array, and only an array")
                            .fix(format!("declare it as `array.{}`, or write a single value.", ty.written())),
                    );
                    return None;
                };
                if let Some(wanted) = of.length()
                    && wanted != items.len()
                {
                    self.error(
                        Diagnostic::new("E0137", format!(
                            "this holds {} where `{}` holds {wanted}.",
                            items.len(),
                            ty.written()
                        ))
                        .primary(*span, "written here")
                        .rule("a fixed array is written with exactly as many elements as it holds")
                        .tip("a shaped array is written flat, row by row: `array.2x3` takes six.")
                        .fix(format!("write {wanted} of them, or let the array grow.")),
                    );
                    return None;
                }
                let mut checked = Vec::with_capacity(items.len());
                for item in items {
                    checked.push(self.expr(item, Some(of.element))?);
                }
                Some(ir::Expr::NewArray { ty, items: checked, span: *span })
            }

            AExpr::Index { array, at, span } => {
                let array = self.expr(array, None)?;
                let held = array.ty();
                let Some(of) = held.array() else {
                    self.error(
                        Diagnostic::new("E0138", format!("`{}` is not an array, so it has no elements.", held.written()))
                            .primary(*span, "indexed here")
                            .rule("only an array is indexed")
                            .tip("a bare word before a bracket is a call; a quoted one is an index.")
                            .fix("index an array instead."),
                    );
                    return None;
                };

                // One index per dimension, and a growable array has the one.
                let wanted = of.dims().len().max(1);
                if at.len() != wanted {
                    self.error(
                        Diagnostic::new("E0139", format!(
                            "`{}` takes {} to index it, and {} given here.",
                            held.written(),
                            count_of(wanted, "index").replace("indexs", "indices"),
                            at.len()
                        ))
                        .primary(*span, "indexed here")
                        .rule("an array takes one index for each dimension it has")
                        .fix(format!("give {wanted} of them.")),
                    );
                    return None;
                }

                self.agree(of.element, expected, *span)?;
                let mut checked = Vec::with_capacity(at.len());
                for index in at {
                    let index = self.expr(index, Some(Ty::U32))?;
                    if !index.ty().is_integer() {
                        self.error(
                            Diagnostic::new("E0140", format!("an index cannot be `{}`.", index.ty().written()))
                                .primary(index.span(), "used as an index here")
                                .rule("an index is a whole number")
                                .tip("the first element is 1, and 0 is no element at all.")
                                .fix("use a whole number."),
                        );
                        return None;
                    }
                    checked.push(index);
                }
                Some(ir::Expr::At {
                    array: Box::new(array),
                    at: checked,
                    ty: of.element,
                    span: *span,
                })
            }

            AExpr::Binary { op, lhs, rhs, span } => {
                // Whichever side knows what it is goes first and tells the other, so both
                // `'x' + 1` and `1 + 'x'` read the 1 as whatever `'x'` is.
                let (lhs, rhs) = self.two_sides(lhs, rhs, expected)?;
                let ty = lhs.ty();
                if !ty.is_number() {
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

        if ty == Ty::Er {
            return match luarust_num::Exact::parse(text) {
                Some(value) => {
                    Some(ir::Expr::Const(Value::Exact(std::rc::Rc::new(value))))
                }
                None => {
                    self.error(
                        Diagnostic::new("E0217", format!("`{text}` is not an exact number."))
                            .primary(at, "written here")
                            .rule("an `er` is a whole number, a decimal, or one number over another")
                            .tip("`|1/3|` is exactly a third, which is why the fraction form is there: no decimal could have written it.")
                            .fix("write something like `|3|`, `|-2.5|` or `|1/3|`."),
                    );
                    None
                }
            };
        }

        if let Some(fmt) = luarust_core::value::decimal_of(ty) {
            return match luarust_num::decimal::text::from_text(fmt, Round::TiesToEven, false, text) {
                Ok(bits) => Some(ir::Expr::Const(Value::float(ty, bits))),
                Err(_) => {
                    self.error(
                        Diagnostic::new("E0218", format!("`{text}` is not a `{}`.", ty.word()))
                            .primary(at, "written here")
                            .rule("a decimal float is written the way a number is written")
                            .tip("`19.99` is exactly nineteen pounds ninety-nine here, which is why the type exists.")
                            .fix("write a number, as in `|19.99|` or `|1.5e3|`."),
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
        AExpr::TypedLiteral { .. } => true,
        // These three answer `bool` whatever is in them, so they need telling nothing.
        AExpr::Compare { .. } | AExpr::Logic { .. } | AExpr::Not { .. } => true,
        // An element's type comes from the array, which knew it already.
        AExpr::Index { .. } => true,
        // A list of values says nothing about what kind of array it is.
        AExpr::Items { .. } => false,
        // And a call answers whatever its function was declared to answer.
        AExpr::Call { .. } => true,
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

/// Whether a block always leaves by a `return`, however it is entered.
///
/// Conservative on purpose: it only says yes where it can see one. A loop that certainly
/// runs, or an `if` whose condition is always true, will not convince it -- and being
/// told to add a `return` that never runs is a smaller cost than a function quietly
/// falling off its own end.
fn always_returns(stmts: &[ir::Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        ir::Stmt::Return { .. } => true,
        // Every arm and the `else`, or one of them might not answer.
        ir::Stmt::If { arms, otherwise, .. } => {
            !otherwise.is_empty()
                && arms.iter().all(|arm| always_returns(&arm.body))
                && always_returns(otherwise)
        }
        _ => false,
    })
}

/// `1 parameter` or `3 parameters`, so a sentence reads either way.
fn count_of(n: usize, thing: &str) -> String {
    if n == 1 { format!("1 {thing}") } else { format!("{n} {thing}s") }
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
        clean("loop.temp.range.ui8 ['i'] = [|1|, |5|] {\n print['i' \\n];\n}\n");
        clean(
            "var.local.mut.ui32 ['total'] = [|0|];\n\
             loop.temp.range.ui32 ['i'] = [|1|, |10|] { handback 'i' as 'total'; }\n\
             print[\"total is \" 'total' \\n];\n",
        );
        clean("var.local.b16 ['a'] = [|0.1|];\nprint['a' \\n];\n");
        clean(
            "var.local.mut.ui64 ['sum'] = [|0|];\n\
             var.local.b64 ['start'] = [time.now];\n\
             loop.temp.range.ui64 ['i'] = [|1|, |100|] {\n\
                 set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];\n\
             }\n\
             var.local.b64 ['elapsed'] = [math { time.now - 'start' }];\n\
             print['sum' \" in \" 'elapsed' \" seconds\\n\"];\n",
        );
    }

    #[test]
    fn a_name_that_was_never_declared_is_reported_with_a_guess() {
        let (_, errors) = run("var.local.b16 ['count'] = [|1|];\nprint['cont'];");
        assert_eq!(errors[0].code, "E0208");
        assert!(errors[0].tips[0].contains("'count'"), "{:?}", errors[0].tips);
    }

    #[test]
    fn a_restricted_variable_cannot_be_touched() {
        // Said by saying nothing, which is the default.
        let (_, errors) = run("var.b16 ['x'] = [|1|];\nprint['x'];");
        assert_eq!(errors[0].code, "E0207");
        // And said out loud.
        assert_eq!(codes("var.restricted.b16 ['x'] = [|1|];\nprint['x'];"), ["E0207"]);
        // The declaration itself is fine either way.
        clean("var.b16 ['x'] = [|1|];");
    }

    #[test]
    fn changing_something_that_never_said_mut_is_reported() {
        assert_eq!(
            codes("var.local.ui32 ['total'] = [|0|];\nset ['total'] = [|55|];"),
            ["E0104"]
        );
        clean("var.local.mut.ui32 ['total'] = [|0|];\nset ['total'] = [|55|];");
    }

    #[test]
    fn the_two_lists_have_to_be_the_same_length() {
        let (_, errors) = run("var.local.b16 ['a', 'b', 'c'] = [|1|, |2|];");
        assert_eq!(errors[0].code, "E0206");
        assert_eq!(errors[0].labels.len(), 2, "both lists are pointed at");
        assert!(errors[0].message.contains("3 names here and 2 values"));
    }

    #[test]
    fn nothing_converts_on_its_own() {
        let (_, errors) = run(
            "var.local.b32 ['a'] = [|1|];\n\
             var.local.b64 ['b'] = [math { 'a' + 1 }];",
        );
        assert_eq!(errors[0].code, "E0215");
        assert!(errors[0].message.contains("`b32` where `b64` was expected"));
    }

    #[test]
    fn a_value_that_will_not_fit_its_type_is_reported() {
        let (_, errors) = run("var.local.ui8 ['small'] = [|300|];");
        assert_eq!(errors[0].code, "E0217");
        assert!(errors[0].tips[0].contains("0 to 255"), "{:?}", errors[0].tips);

        // A fraction has nowhere to go in a whole number.
        assert_eq!(codes("var.local.i32 ['x'] = [|1.5|];"), ["E0218"]);
        // But a float takes it, and one too large becomes an infinity rather than an error.
        clean("var.local.b16 ['x'] = [|70000|];");
    }

    #[test]
    fn every_type_the_language_names_can_be_declared_and_used() {
        // There is no longer any such thing as a type that is designed and not built, so
        // this is the list that says so -- and the one that will fail if a type is ever
        // added to the tower before it works.
        for ty in [
            "b16", "b32", "b64", "b128", "b256", "d32", "d64", "d128", "er", "i8", "i16",
            "i32", "i64", "ui8", "ui16", "ui32", "ui64",
        ] {
            clean(&format!("var.local.{ty} ['x'] = [|1|];\nprint[math {{ 'x' + {ty} |1| }}];"));
        }
        clean("var.local.bool ['b'] = [|true|]; print['b'];");
        clean("var.local.str ['s'] = [|hi|]; print['s'];");
    }

    #[test]
    fn an_exact_rational_is_read_as_written() {
        clean("var.local.er ['third'] = [|1/3|];");
        clean("var.local.er ['half'] = [|-0.5|];");
        clean("var.local.er ['three'] = [|3|];");
        // A percentage is a fraction of a hundred, which this holds exactly -- `20%` is
        // one fifth here rather than the nearest float to it.
        clean("var.local.er ['fifth'] = [math { 20% }];");
        // And anything that is not a number says so rather than becoming one.
        assert_eq!(codes("var.local.er ['x'] = [|hello|];"), ["E0217"]);
        assert_eq!(codes("var.local.er ['x'] = [|1/0|];"), ["E0217"]);
    }

    #[test]
    fn the_visibility_default_can_be_made_an_error() {
        clean("var.b16 ['x'] = [|1|];");
        let (_, errors) = run("defaults.no-visibility-stated.error;\nvar.b16 ['x'] = [|1|];");
        assert_eq!(errors[0].code, "E0202");
        // And it applies to declarations written above it, too.
        assert_eq!(
            codes("var.b16 ['x'] = [|1|];\ndefaults.no-visibility-stated.error;"),
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
        assert_eq!(clean("var.local.b16 ['x'] = [|1|];").overflow, Overflow::Wrap);
        assert_eq!(clean("defaults.overflow.trap;").overflow, Overflow::Trap);
    }

    #[test]
    fn a_printed_value_needs_a_type_from_somewhere() {
        // A bare literal in a print list has nothing to say how to read it.
        let (_, errors) = run("print[math { 1 + 2 }];");
        assert_eq!(errors[0].code, "E0214");
        // A variable brings its own type, so this is fine.
        clean("var.local.b16 ['x'] = [|1|];\nprint[math { 'x' + 1 }];");
    }

    #[test]
    fn a_temp_counter_is_gone_afterwards_and_a_perm_one_is_not() {
        assert_eq!(
            codes("loop.temp.range.ui8 ['i'] = [|1|,|5|] { }\nprint['i'];"),
            ["E0208"]
        );
        clean("loop.perm.range.ui8 ['i'] = [|1|,|5|] { }\nprint['i'];");
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
        let (_, errors) = run("var.local.b16 ['x'] = [|1|];\nvar.local.b16 ['x'] = [|2|];");
        assert_eq!(errors[0].code, "E0203");
        // A block of its own is a different matter.
        clean("var.local.b16 ['x'] = [|1|];\nloop.temp.range.ui8 ['i'] = [|1|,|2|] { var.local.b16 ['x'] = [|2|]; }");
    }

    #[test]
    fn handback_needs_both_sides_to_agree() {
        let (_, errors) = run(
            "var.local.mut.ui32 ['total'] = [|0|];\n\
             var.local.b16 ['bit'] = [|1|];\n\
             handback 'bit' as 'total';",
        );
        assert_eq!(errors[0].code, "E0204");
    }

    #[test]
    fn every_name_becomes_a_slot_and_every_literal_a_value() {
        let checked = clean(
            "var.local.mut.ui32 ['total'] = [|0|];\n\
             loop.temp.range.ui32 ['i'] = [|1|, |10|] { handback 'i' as 'total'; }\n",
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
            "var.local.ui8 ['small'] = [|300|];\n\
             var.local.b16 ['x'] = [|1|];\n\
             set ['x'] = [|2|];\n\
             print['nope'];\n",
        );
        assert_eq!(
            errors.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            ["E0217", "E0104", "E0208"]
        );
    }
    #[test]
    fn a_condition_has_to_be_a_question() {
        clean("if [|true|] { print[\"y\"]; }");
        // A name in quotes is a name here too, so a bool on its own is a whole condition.
        clean("var.local.bool ['f'] = [|true|]; if ['f'] { print[\"y\"]; }");
        clean("var.local.i32 ['n'] = [|1|]; if [math { 'n' > i32 |0| }] { print[\"y\"]; }");
        // A number is not a question, however true it looks in other languages.
        assert_eq!(codes("var.local.i32 ['n'] = [|1|]; if ['n'] { print[\"y\"]; }"), ["E0221"]);
        assert_eq!(codes("var.local.str ['s'] = [|hi|]; if ['s'] { print[\"y\"]; }"), ["E0221"]);
    }

    #[test]
    fn a_value_slot_can_simply_name_a_variable() {
        // Bars for what is written, quotes for what is named -- so copying one variable
        // into another needs no `math` wrapped around a single name.
        let checked = clean("var.local.i32 ['a'] = [|5|];\nvar.local.i32 ['b'] = ['a'];");
        assert!(matches!(&checked.stmts[1], ir::Stmt::Store { value: ir::Expr::Load { .. }, .. }));
        // And the types still have to agree, since nothing converts on its own.
        assert_eq!(
            codes("var.local.i32 ['a'] = [|5|];\nvar.local.b64 ['b'] = ['a'];"),
            ["E0215"]
        );
    }

    #[test]
    fn and_or_and_not_take_questions_and_answer_one() {
        clean("var.local.bool ['f'] = [math { bool |true| and not bool |false| }];");
        clean("var.local.i32 ['n'] = [|1|];\nvar.local.bool ['f'] = [math { 'n' > i32 |0| or 'n' < i32 |9| }];");
        // A number on either side of `and` is not a question either.
        assert_eq!(codes("var.local.i32 ['n'] = [|1|]; var.local.bool ['f'] = [math { 'n' and bool |true| }];"), ["E0221"]);
        assert_eq!(codes("var.local.i32 ['n'] = [|1|]; var.local.bool ['f'] = [math { not 'n' }];"), ["E0221"]);
    }

    #[test]
    fn an_arm_keeps_what_it_declares_to_itself() {
        // Declared inside, used outside: the name is gone at the closing brace, exactly
        // as it is in a loop body.
        assert_eq!(
            codes("if [|true|] { var.local.i32 ['x'] = [|1|]; }\nprint['x'];"),
            ["E0208"]
        );
    }

    #[test]
    fn a_call_to_something_that_is_not_there_is_reported_with_a_guess() {
        let (_, errors) = run("fn.local.i32 ['double'] [i32 'n'] { return 'n'; }\nprint[doubel[|1|]];");
        assert_eq!(errors[0].code, "E0128");
        assert!(errors[0].fixes[0].contains("double"), "{:?}", errors[0].fixes);
        // A near-miss on a statement keyword is guessed at too, since the list of
        // functions could never have offered it.
        let (_, errors) = run("prnt['x'];");
        assert_eq!(errors[0].code, "E0128");
        assert!(errors[0].fixes[0].contains("print"), "{:?}", errors[0].fixes);
    }

    #[test]
    fn a_function_is_checked_against_its_signature() {
        clean("fn.local.i32 ['f'] [i32 'a'] { return 'a'; }\nprint[f[|1|]];");
        // Too few, too many, and the wrong type.
        assert_eq!(codes("fn.local.i32 ['f'] [i32 'a'] { return 'a'; }\nprint[f[]];"), ["E0130"]);
        assert_eq!(codes("fn.local.i32 ['f'] [i32 'a'] { return 'a'; }\nprint[f[|1|, |2|]];"), ["E0130"]);
        assert_eq!(
            codes("fn.local.i32 ['f'] [i32 'a'] { return 'a'; }\nvar.local.b64 ['x'] = [f[|1|]];"),
            ["E0215"]
        );
    }

    #[test]
    fn a_function_answers_on_every_path_or_says_why_not() {
        clean("fn.local.i32 ['f'] [i32 'a'] { if [|true|] { return 'a'; } return 'a'; }");
        clean("fn.local.i32 ['f'] [i32 'a'] { if [|true|] { return 'a'; } else { return 'a'; } }");
        // An `if` with no `else` leaves a path where nothing was answered.
        assert_eq!(codes("fn.local.i32 ['f'] [i32 'a'] { if [|true|] { return 'a'; } }"), ["E0124"]);
        // And a `nothing` function needs no return at all.
        clean("fn.local.nothing ['f'] [] { print[\"hi\"]; }");
    }

    #[test]
    fn a_function_sees_its_parameters_and_nothing_of_its_caller() {
        clean("fn.local.i32 ['f'] [i32 'n'] { return 'n'; }");
        // `outside` is declared at the top level, and a function is not inside it.
        let (_, errors) = run("var.local.i32 ['outside'] = [|1|];\nfn.local.i32 ['f'] [] { return 'outside'; }");
        assert_eq!(errors[0].code, "E0208");
    }

    #[test]
    fn return_belongs_to_a_function() {
        assert_eq!(codes("return |1|;"), ["E0125"]);
        assert_eq!(codes("fn.local.nothing ['f'] [] { return |1|; }"), ["E0126"]);
        assert_eq!(codes("fn.local.i32 ['f'] [] { return; }"), ["E0127", "E0124"]);
    }

    #[test]
    fn break_belongs_to_a_loop() {
        clean("loop.while [|true|] { break; }");
        clean("loop.temp.range.ui8 ['i'] = [|1|, |3|] { break; }");
        // An `if` inside a loop is still inside the loop.
        clean("loop.while [|true|] { if [|true|] { break; } }");
        assert_eq!(codes("break;"), ["E0133"]);
        // A function is not inside whatever loop its declaration sits in.
        assert_eq!(
            codes("loop.while [|true|] { }\nfn.local.nothing ['f'] [] { break; }"),
            ["E0133"]
        );
    }

    #[test]
    fn break_when_reached_needs_a_counter_to_compare() {
        clean("loop.temp.while.ui8 ['n'] [|true|] { break when reached |3|; }");
        clean("loop.temp.range.ui8 ['i'] = [|1|, |9|] { break when reached |3|; }");
        // A `while` loop that named no counter has nothing to have reached anything.
        assert_eq!(codes("loop.while [|true|] { break when reached |3|; }"), ["E0134"]);
    }

    #[test]
    fn a_while_loops_condition_is_a_question_like_any_other() {
        clean("var.local.bool ['f'] = [|true|]; loop.while ['f'] { break; }");
        assert_eq!(codes("var.local.i32 ['n'] = [|1|]; loop.while ['n'] { break; }"), ["E0221"]);
    }

}

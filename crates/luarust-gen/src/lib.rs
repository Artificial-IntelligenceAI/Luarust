//! Writing Luarust programs, so that the ways of running one can be made to disagree.
//!
//! A generated program is **type-directed**: it is built from the types outward, so it
//! always compiles. That is the whole point. A program that failed to check would be
//! rejected identically by every execution path and would prove nothing — the interesting
//! programs are the ones that *run*, because those are the ones two implementations can
//! answer differently.
//!
//! What it deliberately never writes:
//!
//! - **`time.now`**, which is not the same twice and so cannot be compared;
//! - **`restricted` variables**, which cannot be used and would waste the program;
//! - **the types that are not built yet**, which would fail to check;
//! - **loops with large bounds**, since a fuzzer that takes a minute per program is a
//!   fuzzer nobody runs.
//!
//! It does write division and remainder by zero, and arithmetic that overflows, because
//! those stop the program — and stopping in the same place for the same reason is exactly
//! as much a thing to check as printing the same number.

use luarust_parse::ast::Ty;

/// A program, and the seed that made it.
pub struct Written {
    pub seed: u64,
    pub source: String,
}

/// Every type a generated program declares variables of.
const DECLARED: [Ty; 19] = [
    Ty::Er,
    Ty::D32,
    Ty::D64,
    Ty::D128,
    Ty::B16,
    Ty::B32,
    Ty::B64,
    Ty::B128,
    Ty::B256,
    Ty::I8,
    Ty::I16,
    Ty::I32,
    Ty::I64,
    Ty::U8,
    Ty::U16,
    Ty::U32,
    Ty::U64,
    Ty::Bool,
    Ty::Str,
];

/// The types arithmetic and loops work in. `bool` cannot be counted in or added to, and
/// neither can `str`.
const TYPES: [Ty; 17] = [
    Ty::Er,
    Ty::D32,
    Ty::D64,
    Ty::D128,
    Ty::B16,
    Ty::B32,
    Ty::B64,
    Ty::B128,
    Ty::B256,
    Ty::I8,
    Ty::I16,
    Ty::I32,
    Ty::I64,
    Ty::U8,
    Ty::U16,
    Ty::U32,
    Ty::U64,
];

/// Write one program.
pub fn program(seed: u64) -> Written {
    let mut writer = Writer {
        rng: Rng::new(seed),
        scope: Vec::new(),
        funcs: Vec::new(),
        source: String::new(),
        depth: 0,
        names: 0,
        inside: None,
        loops: 0,
        calls: 0,
    };
    writer.program();
    Written { seed, source: writer.source }
}

/// A variable the program has already declared.
#[derive(Clone)]
struct Known {
    name: String,
    ty: Ty,
    mutable: bool,
    /// How deep the block was that declared it, so a loop's variables can be forgotten.
    depth: usize,
    /// How long a growable array was made, which its type does not say. A shaped one's
    /// length is in its type, and nothing else has one.
    length: Option<u32>,
}

/// A function the program has already written.
#[derive(Clone)]
struct Signature {
    name: String,
    params: Vec<Ty>,
    /// `None` when it answers nothing.
    returns: Option<Ty>,
    /// Whether it calls itself, counting its first argument down.
    recursive: bool,
}

struct Writer {
    rng: Rng,
    scope: Vec<Known>,
    /// Every function written so far. A body may only call these, which is what keeps a
    /// generated program from recursing by accident and never coming back.
    funcs: Vec<Signature>,
    source: String,
    depth: usize,
    names: usize,
    /// What the function being written answers, when one is being written.
    inside: Option<Option<Ty>>,
    /// How many loops deep the statement being written is, so a `break` is only written
    /// where there is something to break out of.
    loops: usize,
    /// How many calls deep the expression being written is. A call's arguments are values
    /// like any other, so they may be calls too -- and without a limit here the generator
    /// writes `f0[f1[f2[…` until it runs out of its own stack.
    calls: usize,
}

impl Writer {
    fn program(&mut self) {
        // Functions first in the file, though they need not be: every signature is read
        // before any body, so order is only a convenience here.
        let count = self.rng.below(4);
        for _ in 0..count {
            self.func_decl();
        }
        // One that calls itself, sometimes. Written from a template rather than at
        // random, because a randomly recursive function mostly does not come back.
        if self.rng.below(4) == 0 {
            self.recursive_func();
        }

        // A handful of variables to work with, then things done to them.
        let openers = 2 + self.rng.below(4);
        for _ in 0..openers {
            self.declaration();
        }
        let statements = 2 + self.rng.below(6);
        for _ in 0..statements {
            self.statement();
        }
        // Always print something, so that two paths agreeing is a claim about output
        // rather than about silence.
        self.print();
    }

    fn statement(&mut self) {
        match self.rng.below(10) {
            0..=2 => self.declaration(),
            3..=4 => self.assignment(),
            5 => self.handback(),
            6 if self.depth < 2 => self.loop_stmt(),
            7 if self.depth < 2 => self.if_stmt(),
            8 if !self.funcs.is_empty() => self.call_stmt(),
            9 if self.depth < 2 => self.while_loop(),
            _ if self.rng.below(8) == 0 => self.element_assignment(),
            _ if self.loops > 0 && self.rng.below(6) == 0 => self.break_stmt(),
            _ => self.print(),
        }
    }

    /// `if`, sometimes with `else-if`s, sometimes with an `else`.
    ///
    /// Every arm gets a body, because an arm that is never taken and does nothing proves
    /// nothing about the two ways of running it.
    fn if_stmt(&mut self) {
        let arms = 1 + self.rng.below(3);
        for n in 0..arms {
            let word = if n == 0 { "if" } else { "} else-if" };
            let condition = self.condition(0);
            self.line(&format!("{word} [math {{ {condition} }}] {{"));
            self.arm_body();
        }

        if self.rng.below(2) == 0 {
            self.line("} else {");
            self.arm_body();
        }
        self.line("}");
    }

    /// The inside of one arm. Whatever it declares is gone at the closing brace.
    fn arm_body(&mut self) {
        self.depth += 1;
        let body = 1 + self.rng.below(3);
        for _ in 0..body {
            self.statement();
        }
        self.depth -= 1;
        let depth = self.depth;
        self.scope.retain(|known| known.depth <= depth);
    }

    /// A function that answers a value, or nothing, and calls only functions already
    /// written -- so a generated program always finishes.
    fn func_decl(&mut self) {
        let returns = if self.rng.below(5) == 0 { None } else { Some(self.pick_type()) };
        let name = format!("f{}", self.funcs.len());
        let count = self.rng.below(3);
        let params: Vec<Ty> = (0..count).map(|_| self.pick_type()).collect();

        let chain = match returns {
            Some(ty) => format!("local.{}", ty.word()),
            None => "local.nothing".to_string(),
        };
        let written: Vec<String> = params
            .iter()
            .enumerate()
            .map(|(n, ty)| format!("{} 'p{n}'", ty.written()))
            .collect();
        self.line(&format!("fn.{chain} ['{name}'] [{}] {{", written.join(", ")));

        // A body sees its parameters and nothing of the program around it.
        let outer_scope = std::mem::take(&mut self.scope);
        let outer_depth = std::mem::replace(&mut self.depth, 1);
        let outer_inside = self.inside.replace(returns);
        let outer_loops = std::mem::take(&mut self.loops);
        for (n, ty) in params.iter().enumerate() {
            self.scope.push(Known { name: format!("p{n}"), ty: *ty, mutable: false, depth: 1, length: None });
        }

        let statements = self.rng.below(3);
        for _ in 0..statements {
            self.statement();
        }
        match returns {
            Some(ty) => {
                let value = self.value_of(ty);
                self.line(&format!("return {value};"));
            }
            None if self.rng.below(2) == 0 => self.line("return;"),
            None => {}
        }

        self.scope = outer_scope;
        self.depth = outer_depth;
        self.inside = outer_inside;
        self.loops = outer_loops;
        self.line("}");

        self.funcs.push(Signature { name, params, returns, recursive: false });
    }

    /// One that calls itself, counted down by a `ui8` so it certainly stops.
    fn recursive_func(&mut self) {
        let ty = self.pick_type();
        let name = format!("f{}", self.funcs.len());
        let base = self.literal(ty);
        // Written before the body, so the body may call it -- which is the point.
        self.funcs.push(Signature {
            name: name.clone(),
            params: vec![Ty::U8],
            returns: Some(ty),
            recursive: true,
        });

        self.line(&format!("fn.local.{} ['{name}'] [ui8 'p0'] {{", ty.word()));
        self.line(&format!("    if [math {{ 'p0' = ui8 |0| }}] {{ return |{base}|; }}"));
        self.line(&format!("    return {name}[math {{ 'p0' - ui8 |1| }}];"));
        self.line("}");
    }

    /// A call to something already written that answers this type, if there is one.
    fn call_of(&mut self, ty: Ty) -> Option<String> {
        if self.calls >= 2 {
            return None;
        }
        let usable: Vec<Signature> =
            self.funcs.iter().filter(|f| f.returns == Some(ty)).cloned().collect();
        if usable.is_empty() {
            return None;
        }
        let chosen = usable[self.rng.below(usable.len() as u64) as usize].clone();
        self.calls += 1;
        let args: Vec<String> = chosen
            .params
            .clone()
            .into_iter()
            .enumerate()
            .map(|(n, param)| {
                // The one that calls itself counts its argument down to nothing, so a
                // large one would be a deep recursion and a slow fuzzer rather than a
                // more interesting program.
                if chosen.recursive && n == 0 {
                    format!("|{}|", self.rng.below(12))
                } else {
                    self.value_of(param)
                }
            })
            .collect();
        self.calls -= 1;
        Some(format!("{}[{}]", chosen.name, args.join(", ")))
    }

    /// `greet[…];` — a function that answers nothing, called for what it does.
    fn call_stmt(&mut self) {
        let usable: Vec<Signature> =
            self.funcs.iter().filter(|f| f.returns.is_none()).cloned().collect();
        if usable.is_empty() {
            self.print();
            return;
        }
        let chosen = usable[self.rng.below(usable.len() as u64) as usize].clone();
        self.calls += 1;
        let args: Vec<String> = chosen
            .params
            .clone()
            .into_iter()
            .map(|param| self.value_of(param))
            .collect();
        self.calls -= 1;
        self.line(&format!("{}[{}];", chosen.name, args.join(", ")));
    }

    fn declaration(&mut self) {
        // One declaration in five is an array, which is often enough to be everywhere and
        // seldom enough that most programs are still about the scalars.
        if self.rng.below(5) == 0 {
            self.array_declaration();
            return;
        }
        let ty = DECLARED[self.rng.below(DECLARED.len() as u64) as usize];
        let mutable = self.rng.below(2) == 0;
        let name = self.fresh_name();
        let value = self.value_of(ty);
        let chain = if mutable { format!("local.mut.{}", ty.written()) } else { format!("local.{}", ty.written()) };
        self.line(&format!("var.{chain} ['{name}'] = [{value}];"));
        self.scope.push(Known { name, ty, mutable, depth: self.depth, length: None });
    }

    /// An array: fixed, shaped, or growable, written out or filled.
    fn array_declaration(&mut self) {
        // Half the time from the whole tower, half from the handful of types the rest of
        // a program is most likely to want, so that indexing actually finds a home.
        let element = if self.rng.below(2) == 0 {
            self.pick_type()
        } else {
            const COMMON: [Ty; 6] = [Ty::U8, Ty::U32, Ty::I32, Ty::I64, Ty::B32, Ty::B64];
            COMMON[self.rng.below(COMMON.len() as u64) as usize]
        };
        let name = self.fresh_name();
        let mutable = self.rng.below(2) == 0;

        // A small one. A generated program that makes a thousand-element array is a
        // fuzzer spending its time on memcpy rather than on disagreement.
        let mut grown = None;
        let (chain, ty, value) = match self.rng.below(3) {
            0 => {
                let len = 1 + self.rng.below(4) as u32;
                let items: Vec<String> =
                    (0..len).map(|_| format!("|{}|", self.literal(element))).collect();
                (
                    format!("array.{len}.{}", element.written()),
                    luarust_core::ty::fixed(element, &[len]),
                    format!("[{}]", items.join(", ")),
                )
            }
            1 => {
                let (rows, cols) = (1 + self.rng.below(3) as u32, 1 + self.rng.below(3) as u32);
                let items: Vec<String> =
                    (0..rows * cols).map(|_| format!("|{}|", self.literal(element))).collect();
                (
                    format!("array.{rows}x{cols}.{}", element.written()),
                    luarust_core::ty::fixed(element, &[rows, cols]),
                    format!("[{}]", items.join(", ")),
                )
            }
            _ => {
                let len = 1 + self.rng.below(4) as u32;
                grown = Some(len);
                let fill = self.literal(element);
                (
                    format!("array.{}", element.written()),
                    luarust_core::ty::growable(element),
                    format!("filled[|{len}|, |{fill}|]"),
                )
            }
        };

        let Some(ty) = ty else { return };
        let chain = if mutable { format!("local.mut.{chain}") } else { format!("local.{chain}") };
        self.line(&format!("var.{chain} ['{name}'] = [{value}];"));
        self.scope.push(Known { name, ty, mutable, depth: self.depth, length: grown });
    }

    /// An array in scope holding this, with how it is indexed.
    fn array_of(&mut self, element: Ty) -> Option<(Known, String)> {
        let usable: Vec<Known> = self
            .scope
            .iter()
            .filter(|known| known.ty.array().is_some_and(|of| of.element == element))
            .cloned()
            .collect();
        if usable.is_empty() {
            return None;
        }
        let chosen = usable[self.rng.below(usable.len() as u64) as usize].clone();
        let at = self.indices_for(&chosen);
        Some((chosen, at))
    }

    /// The index list for this array and no other: one index per dimension it has.
    ///
    /// It has to be built from the array it will be written next to. Taking the shape of
    /// whichever array came to hand gives a 2-D array's two indices to a 1-D one, and the
    /// checker is quite right to refuse it.
    fn indices_for(&mut self, array: &Known) -> String {
        let of = array.ty.array().expect("only arrays are indexed");
        // A shaped one's length is in its type; a growable one's is whatever it was made,
        // which only the declaration knew.
        let dims: Vec<u32> =
            if of.dims().is_empty() { vec![array.length.unwrap_or(0)] } else { of.dims().to_vec() };

        // Mostly in range, sometimes not: an index past the end is a fault, and every
        // path has to agree about it just as much as about an answer.
        let written: Vec<String> = dims
            .iter()
            .map(|size| {
                if *size == 0 || self.rng.below(8) == 0 {
                    format!("|{}|", self.rng.below(4))
                } else {
                    format!("|{}|", 1 + self.rng.below(u64::from(*size)))
                }
            })
            .collect();
        format!("[{}]", written.join(", "))
    }

    fn assignment(&mut self) {
        let Some(target) = self.pick_changeable() else {
            self.print();
            return;
        };
        // An `er` inside a loop gets a literal, never an expression.
        //
        // `er` is exact and unbounded, so it never rounds and never overflows -- which
        // means `set ['x'] = [math { (524 - 'x') x (792 - 'x') }]` doubles the length of
        // its numerator and denominator every time round. Sixteen iterations is 65,536
        // times the digits, and the fuzzer sat on one such program for nine seconds.
        // Nothing is wrong with the arithmetic; it is the one type in the language with
        // no ceiling to stop it, so the generator has to supply one.
        let value = if target.ty == Ty::Er && self.depth > 0 {
            format!("|{}|", self.literal(target.ty))
        } else {
            self.value_of(target.ty)
        };
        self.line(&format!("set ['{}'] = [{value}];", target.name));
    }

    fn handback(&mut self) {
        // `handback` adds, and neither truth nor text can be added to.
        let Some(target) = self.pick_changeable().filter(|known| numeric(known.ty)) else {
            self.print();
            return;
        };
        // Both sides must be the same type, since nothing here converts on its own.
        let Some(source) = self.pick_of(target.ty) else {
            self.print();
            return;
        };
        self.line(&format!("handback '{}' as '{}';", source.name, target.name));
    }

    fn loop_stmt(&mut self) {
        // Small bounds. A generated program that counts to a million is a fuzzer that
        // finds one bug an hour.
        let ty = if self.rng.below(4) == 0 { self.pick_type() } else { Ty::U8 };
        let lifetime = if self.rng.below(2) == 0 { "temp" } else { "perm" };
        let name = self.fresh_name();
        let from = self.rng.below(3);
        let to = from + self.rng.below(4);

        self.line(&format!(
            "loop.{lifetime}.range.{} ['{name}'] = [|{from}|, |{to}|] {{",
            ty.word()
        ));

        self.depth += 1;
        self.loops += 1;
        let counter = Known { name: name.clone(), ty, mutable: false, depth: self.depth, length: None };
        self.scope.push(counter.clone());

        let body = 1 + self.rng.below(3);
        for _ in 0..body {
            self.statement();
        }

        self.depth -= 1;
        self.loops -= 1;
        // Everything the body declared is out of reach now. A `perm` counter is not.
        let depth = self.depth;
        self.scope.retain(|known| known.depth <= depth);
        if lifetime == "perm" {
            self.scope.push(Known { depth, ..counter });
        }

        self.line("}");
    }

    /// A loop that runs while something holds -- and always stops, because the last thing
    /// in its body counts its own passes and leaves. A generated condition that happened
    /// never to become false would otherwise be a fuzzer that never finishes.
    fn while_loop(&mut self) {
        let lifetime = if self.rng.below(2) == 0 { "temp" } else { "perm" };
        let name = self.fresh_name();
        let condition = self.condition(0);
        self.line(&format!("loop.{lifetime}.while.ui8 ['{name}'] [math {{ {condition} }}] {{"));

        self.depth += 1;
        self.loops += 1;
        let counter = Known { name: name.clone(), ty: Ty::U8, mutable: false, depth: self.depth, length: None };
        self.scope.push(counter.clone());

        let body = self.rng.below(3);
        for _ in 0..body {
            self.statement();
        }
        let passes = 1 + self.rng.below(6);
        self.line(&format!("break when reached |{passes}|;"));

        self.depth -= 1;
        self.loops -= 1;
        let depth = self.depth;
        self.scope.retain(|known| known.depth <= depth);
        if lifetime == "perm" {
            self.scope.push(Known { depth, ..counter });
        }
        self.line("}");
    }

    fn break_stmt(&mut self) {
        self.line("break;");
    }

    fn print(&mut self) {
        let mut items = String::new();
        let count = 1 + self.rng.below(3);
        for _ in 0..count {
            if self.rng.below(3) == 0 || self.scope.is_empty() {
                items.push_str(&format!("\"{}\" ", self.rng.below(1000)));
            } else {
                let known = self.scope[self.rng.below(self.scope.len() as u64) as usize].clone();
                // A whole array prints as itself, but an element and a length are worth
                // printing too, and this is where either is cheapest to reach.
                if known.ty.array().is_some() {
                    match self.rng.below(3) {
                        0 => {
                            items.push_str(&format!("count['{}'] ", known.name));
                            continue;
                        }
                        1 => {
                            let at = self.indices_for(&known);
                            items.push_str(&format!("'{}'{at} ", known.name));
                            continue;
                        }
                        _ => {}
                    }
                }
                items.push_str(&format!("'{}' ", known.name));
            }
        }
        self.line(&format!("print[{items}\\n];"));
    }

    // ---- values -----------------------------------------------------------------

    /// Something of exactly this type, in a value slot.
    fn value_of(&mut self, ty: Ty) -> String {
        // A truth is either written down or worked out by comparing two numbers, which is
        // the only way the language makes one.
        //
        // One side has to be a variable. A comparison says nothing about what its sides
        // are, so `math { 1 < 2 }` has no type in reach and does not compile -- which is
        // the language being consistent, and is also why this cannot just write two
        // literals and hope.
        // An array's value is a whole array, written out or filled.
        if let Some(of) = ty.array() {
            let element = of.element;
            return match of.length() {
                Some(len) => {
                    let items: Vec<String> =
                        (0..len).map(|_| format!("|{}|", self.literal(element))).collect();
                    format!("[{}]", items.join(", "))
                }
                None => {
                    let len = self.rng.below(5);
                    format!("filled[|{len}|, |{}|]", self.literal(element))
                }
            };
        }

        // An element read straight out, with no math block around it. Most values are a
        // bare literal, so waiting for one to be built inside `math { }` leaves indexing
        // far rarer than the risk in it deserves.
        if self.rng.below(3) == 0
            && let Some((array, at)) = self.array_of(ty)
        {
            return format!("'{}'{at}", array.name);
        }

        // A call is a value like any other, wherever one of its type is wanted.
        if self.rng.below(4) == 0
            && let Some(call) = self.call_of(ty)
        {
            return call;
        }
        if ty == Ty::Bool && self.rng.below(2) == 0 {
            return format!("math {{ {} }}", self.condition(0));
        }
        if ty == Ty::Bool || ty == Ty::Str || self.rng.below(3) != 0 {
            return format!("|{}|", self.literal(ty));
        }
        format!("math {{ {} }}", self.arithmetic(ty, 0))
    }

    /// A `bool` expression, as it is written inside a math block.
    ///
    /// A truth is either compared into existence or joined out of smaller ones, which is
    /// all the language offers -- there is no other way to make one.
    fn condition(&mut self, depth: usize) -> String {
        if depth < 2 {
            match self.rng.below(6) {
                0 => {
                    let (a, b) = (self.condition(depth + 1), self.condition(depth + 1));
                    return format!("({a}) and ({b})");
                }
                1 => {
                    let (a, b) = (self.condition(depth + 1), self.condition(depth + 1));
                    return format!("({a}) or ({b})");
                }
                2 => return format!("not ({})", self.condition(depth + 1)),
                _ => {}
            }
        }

        // A bool already in scope is the cheapest truth there is.
        if let Some(known) = self.pick_bool().filter(|_| self.rng.below(3) == 0) {
            return format!("'{}'", known.name);
        }

        let op = match self.rng.below(6) {
            0 => "<",
            1 => ">",
            2 => "=",
            // Every spelling has to lex, so all of them get written.
            3 => ["</=", "<=", "≤"][self.rng.below(3) as usize],
            4 => [">/=", ">=", "≥"][self.rng.below(3) as usize],
            // Three spellings of the same thing, all of which have to lex.
            _ => match self.rng.below(3) {
                0 => "!=",
                1 => "not=",
                _ => "≠",
            },
        };

        // Either a variable supplies the type, or both sides say what they are. A
        // comparison tells its sides nothing, so one of those has to happen.
        if let Some(known) = self.pick_numeric().filter(|_| self.rng.below(2) == 0) {
            let other = self.arithmetic(known.ty, 1);
            return format!("'{}' {op} {other}", known.name);
        }
        let operands = self.pick_type();
        format!(
            "{} |{}| {op} {} |{}|",
            operands.word(),
            self.literal(operands),
            operands.word(),
            self.literal(operands)
        )
    }

    /// An expression of exactly this type, inside a math block.
    fn arithmetic(&mut self, ty: Ty, depth: usize) -> String {
        if depth >= 2 || self.rng.below(3) == 0 {
            return self.atom(ty);
        }
        let operator = match self.rng.below(10) {
            0..=2 => "+",
            3..=4 => "-",
            5..=6 => "x",
            7 => "div",
            8 => "mod",
            // Only ever a small whole exponent: a large one overflows into an infinity
            // and a fractional one is refused, and neither says anything interesting.
            _ => return format!("({} ** {})", self.atom(ty), 1 + self.rng.below(3)),
        };
        format!(
            "({} {operator} {})",
            self.arithmetic(ty, depth + 1),
            self.arithmetic(ty, depth + 1)
        )
    }

    fn atom(&mut self, ty: Ty) -> String {
        // An element of an array, where one is in reach. Taken most of the time it is
        // offered rather than now and then: what makes this rare is having an array of
        // the very type wanted, not the die, and rolling against it again buries the
        // indexing the fuzzer is here for.
        if self.rng.below(3) != 0
            && let Some((array, at)) = self.array_of(ty)
        {
            return format!("'{}'{at}", array.name);
        }
        // How many an array holds, which answers in whatever wants it.
        if ty.is_integer()
            && self.rng.below(6) == 0
            && let Some(array) = self.any_array()
        {
            return format!("count['{}']", array.name);
        }
        match self.pick_of(ty) {
            Some(known) if self.rng.below(2) == 0 => format!("'{}'", known.name),
            // A literal that says what it is, which is the only kind that works where
            // nothing else supplies a type.
            _ if self.rng.below(3) == 0 => format!("{} |{}|", ty.word(), self.literal(ty)),
            _ => self.literal(ty),
        }
    }

    /// A value that will fit the type it is being read as.
    fn literal(&mut self, ty: Ty) -> String {
        if ty == Ty::Bool {
            return if self.rng.below(2) == 0 { "true".into() } else { "false".into() };
        }
        if ty == Ty::Str {
            return match self.rng.below(4) {
                0 => "".to_string(),
                1 => format!("text {}", self.rng.below(1000)),
                2 => "🧑‍🧑‍🧒‍🧒".to_string(),
                _ => format!("a phrase, with punctuation {}", self.rng.below(100)),
            };
        }
        if ty == Ty::Er {
            return match self.rng.below(4) {
                0 => format!("{}", self.rng.below(1000)),
                1 => format!("-{}", self.rng.below(1000)),
                2 => format!("{}.{}", self.rng.below(100), self.rng.below(1000)),
                // The form no decimal could have written, which is the whole point of it.
                _ => format!("{}/{}", self.rng.below(1000), 1 + self.rng.below(999)),
            };
        }
        if ty.is_float() {
            return match self.rng.below(6) {
                0 => "0".to_string(),
                1 => format!("{}", self.rng.below(100)),
                2 => format!("-{}", self.rng.below(100)),
                3 => format!("0.{}", self.rng.below(1000)),
                4 => format!("{}.{}", self.rng.below(50), self.rng.below(100)),
                _ => format!("{}", 1 + self.rng.below(1000)),
            };
        }
        // Whole numbers, kept inside the width so the program checks.
        let width = ty.int_bits().unwrap_or(64);
        let ceiling: i128 = if ty.is_signed() { 1i128 << (width - 1) } else { 1i128 << width };
        let ceiling = ceiling.min(1000) as u64;
        let magnitude = self.rng.below(ceiling.max(2));
        if ty.is_signed() && self.rng.below(3) == 0 {
            format!("-{magnitude}")
        } else {
            format!("{magnitude}")
        }
    }

    // ---- keeping track ----------------------------------------------------------

    fn pick_type(&mut self) -> Ty {
        TYPES[self.rng.below(TYPES.len() as u64) as usize]
    }

    fn pick_of(&mut self, ty: Ty) -> Option<Known> {
        let matching: Vec<Known> =
            self.scope.iter().filter(|known| known.ty == ty).cloned().collect();
        if matching.is_empty() {
            return None;
        }
        Some(matching[self.rng.below(matching.len() as u64) as usize].clone())
    }

    /// Any variable arithmetic works on, which is what a comparison needs one side to be.
    fn pick_numeric(&mut self) -> Option<Known> {
        let usable: Vec<Known> =
            self.scope.iter().filter(|known| numeric(known.ty)).cloned().collect();
        if usable.is_empty() {
            return None;
        }
        Some(usable[self.rng.below(usable.len() as u64) as usize].clone())
    }

    fn pick_bool(&mut self) -> Option<Known> {
        let usable: Vec<Known> =
            self.scope.iter().filter(|known| known.ty == Ty::Bool).cloned().collect();
        if usable.is_empty() {
            return None;
        }
        Some(usable[self.rng.below(usable.len() as u64) as usize].clone())
    }

    /// Any array at all, for the things that do not mind what is in one.
    fn any_array(&mut self) -> Option<Known> {
        let usable: Vec<Known> =
            self.scope.iter().filter(|known| known.ty.array().is_some()).cloned().collect();
        if usable.is_empty() {
            return None;
        }
        Some(usable[self.rng.below(usable.len() as u64) as usize].clone())
    }

    /// `set ['xs'[…]] = […];` — changing one element.
    fn element_assignment(&mut self) {
        let changeable: Vec<Known> = self
            .scope
            .iter()
            .filter(|known| known.mutable && known.ty.array().is_some())
            .cloned()
            .collect();
        if changeable.is_empty() {
            self.print();
            return;
        }
        let target = changeable[self.rng.below(changeable.len() as u64) as usize].clone();
        let element = target.ty.array().expect("just filtered for one").element;
        let at = self.indices_for(&target);
        // Same ceiling as a plain assignment: see `assignment`.
        let value = if element == Ty::Er && self.depth > 0 {
            format!("|{}|", self.literal(element))
        } else {
            self.value_of(element)
        };
        self.line(&format!("set ['{}'{at}] = [{value}];", target.name));
    }

    fn pick_changeable(&mut self) -> Option<Known> {
        let changeable: Vec<Known> =
            self.scope.iter().filter(|known| known.mutable).cloned().collect();
        if changeable.is_empty() {
            return None;
        }
        Some(changeable[self.rng.below(changeable.len() as u64) as usize].clone())
    }

    fn fresh_name(&mut self) -> String {
        self.names += 1;
        // Occasionally something only Luarust would allow, since names are raw.
        match self.rng.below(12) {
            0 => format!("a name with spaces {}", self.names),
            1 => format!("🧑‍🧑‍🧒‍🧒{}", self.names),
            _ => format!("v{}", self.names),
        }
    }

    fn line(&mut self, text: &str) {
        for _ in 0..self.depth {
            self.source.push_str("    ");
        }
        self.source.push_str(text);
        self.source.push('\n');
    }
}

/// xorshift64*, so a seed always produces the same program and a failure can be looked at
/// again by name.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 { 0 } else { self.next() % bound }
    }
}

/// Whether arithmetic works on this at all. Neither truth nor text can be added.
fn numeric(ty: Ty) -> bool {
    ty.is_integer() || ty.is_float()
}

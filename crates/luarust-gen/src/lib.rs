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

/// Every type a generated program is allowed to use.
const TYPES: [Ty; 13] = [
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
        source: String::new(),
        depth: 0,
        names: 0,
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
}

struct Writer {
    rng: Rng,
    scope: Vec<Known>,
    source: String,
    depth: usize,
    names: usize,
}

impl Writer {
    fn program(&mut self) {
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
            6..=7 if self.depth < 2 => self.loop_stmt(),
            _ => self.print(),
        }
    }

    fn declaration(&mut self) {
        let ty = self.pick_type();
        let mutable = self.rng.below(2) == 0;
        let name = self.fresh_name();
        let value = self.value_of(ty);
        let chain = if mutable { format!("local.mut.{}", ty.word()) } else { format!("local.{}", ty.word()) };
        self.line(&format!("var.{chain} ['{name}'] = [{value}];"));
        self.scope.push(Known { name, ty, mutable, depth: self.depth });
    }

    fn assignment(&mut self) {
        let Some(target) = self.pick_changeable() else {
            self.print();
            return;
        };
        let value = self.value_of(target.ty);
        self.line(&format!("set ['{}'] = [{value}];", target.name));
    }

    fn handback(&mut self) {
        let Some(target) = self.pick_changeable() else {
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
            "loop.{lifetime}.range.{} ['{name}'] = ['{from}', '{to}'] {{",
            ty.word()
        ));

        self.depth += 1;
        let counter = Known { name: name.clone(), ty, mutable: false, depth: self.depth };
        self.scope.push(counter.clone());

        let body = 1 + self.rng.below(3);
        for _ in 0..body {
            self.statement();
        }

        self.depth -= 1;
        // Everything the body declared is out of reach now. A `perm` counter is not.
        let depth = self.depth;
        self.scope.retain(|known| known.depth <= depth);
        if lifetime == "perm" {
            self.scope.push(Known { depth, ..counter });
        }

        self.line("}");
    }

    fn print(&mut self) {
        let mut items = String::new();
        let count = 1 + self.rng.below(3);
        for _ in 0..count {
            if self.rng.below(3) == 0 || self.scope.is_empty() {
                items.push_str(&format!("\"{}\" ", self.rng.below(1000)));
            } else {
                let known = self.scope[self.rng.below(self.scope.len() as u64) as usize].clone();
                items.push_str(&format!("'{}' ", known.name));
            }
        }
        self.line(&format!("print[{items}\\n];"));
    }

    // ---- values -----------------------------------------------------------------

    /// Something of exactly this type, in a value slot.
    fn value_of(&mut self, ty: Ty) -> String {
        if self.rng.below(3) == 0 {
            format!("math {{ {} }}", self.arithmetic(ty, 0))
        } else {
            format!("'{}'", self.literal(ty))
        }
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
        match self.pick_of(ty) {
            Some(known) if self.rng.below(2) == 0 => format!("'{}'", known.name),
            _ => self.literal(ty),
        }
    }

    /// A number that will fit the type it is being read as.
    fn literal(&mut self, ty: Ty) -> String {
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

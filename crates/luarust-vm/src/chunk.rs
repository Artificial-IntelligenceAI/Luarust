//! What a compiled Luarust program is.
//!
//! Registers rather than a stack, the way Lua does it. A stack machine spends much of its
//! time pushing and popping values it already knows where to find, and this language hands
//! the compiler numbered slots for its variables anyway — so the registers are half
//! allocated before compilation starts.

use luarust_core::heap::Collect;
use luarust_core::value::{Division, Engine, Floats, Overflow, Value};
use luarust_diag::Span;
use luarust_core::{BinOp, CmpOp, Ty};

/// Which register. Sixteen bits is far more than any program in this language will want.
pub type Reg = u16;

/// One instruction.
///
/// Every operand is a register or an index; nothing is implicit, and nothing is on a
/// stack, so an instruction says everything about itself.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Op {
    /// Put a constant in a register.
    Const { dst: Reg, konst: u32 },
    /// Copy one register to another. The type is here for the same reason it is on
    /// `Binary`: whatever reads this chunk should not have to work out again what the
    /// checker already knew. The VM could manage without it, since its values carry their
    /// own type; the JIT cannot, because a register is just bits by then.
    Move { dst: Reg, src: Reg, ty: Ty },
    /// The type is here because the checker already knew it. Working it out again from
    /// the values, once per operation, was costing more than the arithmetic did.
    ///
    /// `nonnegative` is only ever true on `Div` and `Mod`, and says the checker proved
    /// the dividend at or above zero and the divisor above it — so floored and
    /// truncated division agree and nothing about the divisor needs guarding. It is
    /// advice for a compiler: the JIT drops its guards on the strength of it, while
    /// the VM ignores it and computes floored `mod` the long way, so a proof that was
    /// wrong is caught by the paths disagreeing rather than believed by both.
    Binary { op: BinOp, ty: Ty, dst: Reg, lhs: Reg, rhs: Reg, nonnegative: bool },
    Neg { dst: Reg, src: Reg, ty: Ty },
    /// Answers `bool`. `operands` is what the two sides are, which is what decides how
    /// they get compared.
    Compare { op: CmpOp, operands: Ty, dst: Reg, lhs: Reg, rhs: Reg },
    /// Read the clock into a register.
    TimeNow { dst: Reg, ty: Ty },
    /// Write a piece of text from the pool.
    PrintText { text: u32 },
    /// Stringify a register and write it.
    PrintValue { src: Reg, ty: Ty },
    /// Call a function. Its arguments are the `argc` registers from `base`, and what it
    /// answers lands in `dst`. `dst` is ignored when the function answers nothing.
    Call { func: u32, base: Reg, argc: u16, dst: Reg },
    /// Leave the function, with the value in `src` when there is one.
    Return { src: Reg, ty: Ty },
    ReturnNothing,
    /// A new array of `count` elements, taken from the registers at `items`.
    NewArray { dst: Reg, items: Reg, count: u16, ty: Ty },
    /// A new array of `length` elements, every one of them what is in `value`.
    Filled { dst: Reg, length: Reg, value: Reg, ty: Ty },
    /// One element of an array, indexed by the `rank` registers at `at`.
    At { dst: Reg, array: Reg, at: Reg, rank: u8, ty: Ty },
    /// Put `value` in one element of an array.
    StoreAt { array: Reg, at: Reg, rank: u8, value: Reg, ty: Ty },
    /// How many elements an array holds.
    Count { dst: Reg, array: Reg, ty: Ty },
    /// Turn a `bool` register around.
    Not { dst: Reg, src: Reg },
    /// Jump if a `bool` register is false.
    JumpIfFalse { cond: Reg, target: u32 },
    /// Jump if a `bool` register is true.
    JumpIfTrue { cond: Reg, target: u32 },
    /// Jump if `lhs` is greater than `rhs`. `operands` is what the two are, which is
    /// what decides how they get compared.
    JumpIfGreater { lhs: Reg, rhs: Reg, ty: Ty, target: u32 },
    /// Jump if the two are equal.
    JumpIfEqual { lhs: Reg, rhs: Reg, ty: Ty, target: u32 },
    Jump { target: u32 },
    Halt,
}

/// A compiled program.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub code: Vec<Op>,
    /// Where each instruction came from, so a fault at run time can still point at source.
    pub spans: Vec<Span>,
    pub consts: Vec<Value>,
    pub texts: Vec<String>,
    /// How many registers the machine needs.
    pub registers: usize,
    pub overflow: Overflow,
    /// What the program does about arrays nothing can reach, and how much of a float it
    /// writes out. Both travel with the program for the same reason `overflow` does: they
    /// are decisions the project made, and `luarust-run` has no project file to read.
    pub collect: Collect,
    pub floats: Floats,
    /// Which engine the project asked for. A build that has not got it runs the VM
    /// instead: a preference that cannot be met is not an error.
    pub engine: Engine,
    /// How hard the project insisted on `engine`. Travels for the same reason `engine`
    /// does: `luarust-run` has no project file, and a requirement nobody carried is not a
    /// requirement.
    pub insistence: luarust_core::value::Insistence,
    /// How a division rounds. One setting for `div` and `mod` together, so the two always
    /// describe the same division.
    pub division: Division,
    /// Every function, each with its own code and its own register file. The constants
    /// and the texts are shared, because they are the same values wherever they appear.
    pub funcs: Vec<Routine>,
}

/// One function's compiled body.
#[derive(Clone, Debug, PartialEq)]
pub struct Routine {
    pub code: Vec<Op>,
    pub spans: Vec<Span>,
    pub registers: usize,
    /// The types of the arguments, which are also the first registers, in order.
    pub params: Vec<Ty>,
    /// What it answers, when it answers anything.
    pub returns: Option<Ty>,
}

impl Chunk {
    /// A listing, in the spirit of `javap -c`. What the compiler actually decided.
    pub fn disassemble(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();

        let _ = writeln!(out, "registers: {}", self.registers);
        if !self.consts.is_empty() {
            let _ = writeln!(out, "constants:");
            for (i, value) in self.consts.iter().enumerate() {
                let _ = writeln!(out, "  k{i:<4} {:<6} {value}", value.ty().word());
            }
        }
        if !self.texts.is_empty() {
            let _ = writeln!(out, "text:");
            for (i, text) in self.texts.iter().enumerate() {
                let _ = writeln!(out, "  t{i:<4} {:?}", text);
            }
        }

        let _ = writeln!(out, "code:");
        for (at, op) in self.code.iter().enumerate() {
            let _ = writeln!(out, "  {at:>4}  {}", self.show(*op));
        }

        for (index, routine) in self.funcs.iter().enumerate() {
            let answers = match routine.returns {
                Some(ty) => format!("answers {}", ty.word()),
                None => "answers nothing".to_string(),
            };
            let written: Vec<&str> = routine.params.iter().map(|ty| ty.word()).collect();
            let _ = writeln!(
                out,
                "\nf{index}: {} registers, taking [{}], {answers}",
                routine.registers,
                written.join(", ")
            );
            for (at, op) in routine.code.iter().enumerate() {
                let _ = writeln!(out, "  {at:>4}  {}", self.show(*op));
            }
        }
        out
    }

    /// One instruction, as a line of the listing.
    fn show(&self, op: Op) -> String {
        match op {
            Op::Const { dst, konst } => {
                format!("const        r{dst}, k{konst}    -- {}", self.consts[konst as usize])
            }
            Op::Move { dst, src, ty } => {
                format!("move         r{dst}, r{src}    -- {}", ty.word())
            }
            Op::Binary { op, ty, dst, lhs, rhs, nonnegative } => {
                let proven = if nonnegative { ", proven nonnegative" } else { "" };
                format!("{:<12} r{dst}, r{lhs}, r{rhs}    -- {}{proven}", name_of(op), ty.word())
            }
            Op::Neg { dst, src, ty } => format!("neg          r{dst}, r{src}    -- {}", ty.word()),
            Op::Not { dst, src } => format!("not          r{dst}, r{src}"),
            Op::NewArray { dst, items, count, ty } => {
                format!("array.new    r{dst}, r{items}..{count}    -- {}", ty.written())
            }
            Op::Filled { dst, length, value, ty } => {
                format!("array.fill   r{dst}, r{length}, r{value}    -- {}", ty.written())
            }
            Op::At { dst, array, at, rank, ty } => {
                format!("array.at     r{dst}, r{array}, r{at}..{rank}    -- {}", ty.written())
            }
            Op::StoreAt { array, at, rank, value, ty } => {
                format!("array.put    r{array}, r{at}..{rank}, r{value}    -- {}", ty.written())
            }
            Op::Count { dst, array, ty } => {
                format!("array.count  r{dst}, r{array}    -- {}", ty.word())
            }
            Op::JumpIfFalse { cond, target } => format!("jump.false   r{cond}, {target}"),
            Op::JumpIfTrue { cond, target } => format!("jump.true    r{cond}, {target}"),
            Op::Compare { op, operands, dst, lhs, rhs } => format!(
                "{:<12} r{dst}, r{lhs}, r{rhs}    -- {}",
                match op {
                    CmpOp::Less => "less",
                    CmpOp::Greater => "greater",
                    CmpOp::Equal => "equal",
                    CmpOp::LessEqual => "less.eq",
                    CmpOp::GreaterEqual => "greater.eq",
                    CmpOp::NotEqual => "not.equal",
                },
                operands.word()
            ),
            Op::TimeNow { dst, ty } => format!("time.now     r{dst}    -- {}", ty.word()),
            Op::PrintText { text } => {
                format!("print.text   t{text}    -- {:?}", self.texts[text as usize])
            }
            Op::PrintValue { src, ty } => format!("print.value  r{src}    -- {}", ty.word()),
            Op::JumpIfGreater { lhs, rhs, ty, target } => {
                format!("jump.gt      r{lhs}, r{rhs}, {target}    -- {}", ty.word())
            }
            Op::JumpIfEqual { lhs, rhs, ty, target } => {
                format!("jump.eq      r{lhs}, r{rhs}, {target}    -- {}", ty.word())
            }
            Op::Jump { target } => format!("jump         {target}"),
            Op::Call { func, base, argc, dst } => {
                format!("call         f{func}, r{base}..{argc}, -> r{dst}")
            }
            Op::Return { src, ty } => format!("return       r{src}    -- {}", ty.word()),
            Op::ReturnNothing => "return".to_string(),
            Op::Halt => "halt".to_string(),
        }
    }
}

fn name_of(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Mod => "mod",
        BinOp::Pow => "pow",
    }
}

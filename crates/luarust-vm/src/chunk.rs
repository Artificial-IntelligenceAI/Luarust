//! What a compiled Luarust program is.
//!
//! Registers rather than a stack, the way Lua does it. A stack machine spends much of its
//! time pushing and popping values it already knows where to find, and this language hands
//! the compiler numbered slots for its variables anyway — so the registers are half
//! allocated before compilation starts.

use luarust_check::value::{Overflow, Value};
use luarust_diag::Span;
use luarust_parse::ast::{BinOp, CmpOp, Ty};

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
    /// Copy one register to another.
    Move { dst: Reg, src: Reg },
    /// The type is here because the checker already knew it. Working it out again from
    /// the values, once per operation, was costing more than the arithmetic did.
    Binary { op: BinOp, ty: Ty, dst: Reg, lhs: Reg, rhs: Reg },
    Neg { dst: Reg, src: Reg },
    /// Answers `bool`. `operands` is what the two sides are, which is what decides how
    /// they get compared.
    Compare { op: CmpOp, operands: Ty, dst: Reg, lhs: Reg, rhs: Reg },
    /// Read the clock into a register.
    TimeNow { dst: Reg, ty: Ty },
    /// Write a piece of text from the pool.
    PrintText { text: u32 },
    /// Stringify a register and write it.
    PrintValue { src: Reg },
    /// Jump if `lhs` is greater than `rhs`.
    JumpIfGreater { lhs: Reg, rhs: Reg, target: u32 },
    /// Jump if the two are equal.
    JumpIfEqual { lhs: Reg, rhs: Reg, target: u32 },
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
            let text = match *op {
                Op::Const { dst, konst } => {
                    format!("const        r{dst}, k{konst}    -- {}", self.consts[konst as usize])
                }
                Op::Move { dst, src } => format!("move         r{dst}, r{src}"),
                Op::Binary { op, ty, dst, lhs, rhs } => {
                    format!("{:<12} r{dst}, r{lhs}, r{rhs}    -- {}", name_of(op), ty.word())
                }
                Op::Neg { dst, src } => format!("neg          r{dst}, r{src}"),
                Op::Compare { op, operands, dst, lhs, rhs } => format!(
                    "{:<12} r{dst}, r{lhs}, r{rhs}    -- {}",
                    match op {
                        CmpOp::Less => "less",
                        CmpOp::Greater => "greater",
                        CmpOp::Equal => "equal",
                    },
                    operands.word()
                ),
                Op::TimeNow { dst, ty } => format!("time.now     r{dst}    -- {}", ty.word()),
                Op::PrintText { text } => {
                    format!("print.text   t{text}    -- {:?}", self.texts[text as usize])
                }
                Op::PrintValue { src } => format!("print.value  r{src}"),
                Op::JumpIfGreater { lhs, rhs, target } => {
                    format!("jump.gt      r{lhs}, r{rhs}, {target}")
                }
                Op::JumpIfEqual { lhs, rhs, target } => {
                    format!("jump.eq      r{lhs}, r{rhs}, {target}")
                }
                Op::Jump { target } => format!("jump         {target}"),
                Op::Halt => "halt".to_string(),
            };
            let _ = writeln!(out, "  {at:>4}  {text}");
        }
        out
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

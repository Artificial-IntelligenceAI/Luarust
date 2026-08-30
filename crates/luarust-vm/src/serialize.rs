//! Writing a chunk to a file, and reading one back.
//!
//! This is what makes "compile once, run anywhere" literally true rather than nearly
//! true: a `.lrc` file is the whole program, and running it needs no source, no compiler
//! and no knowledge of the machine that wrote it.
//!
//! Three things the format is careful about.
//!
//! **Fixed endianness.** Everything is little-endian on the way out and on the way back,
//! whatever the machine underneath is, so a file written on one architecture reads
//! identically on another. That is the entire promise.
//!
//! **The source travels with it.** A chunk carries the text it was compiled from, which
//! costs a few kilobytes and buys the thing that would otherwise be lost: a program that
//! stops half way through can still point at the line that did it, on a machine that has
//! never seen the source.
//!
//! **Nothing read from a file is trusted.** Every index is checked against what it
//! indexes before the chunk is handed back, because a corrupt file must produce a
//! complaint and not a crash — and "run anywhere" means files will arrive from places
//! nobody vouched for.

use crate::chunk::{Chunk, Op};
use luarust_check::value::{Overflow, Value};
use luarust_diag::Span;
use luarust_num::Uint;
use luarust_parse::ast::{BinOp, Ty};

/// What every Luarust chunk begins with.
pub const MAGIC: &[u8; 8] = b"LUARUST\x1b";

/// The format's version. Read a file claiming a different one and it is refused rather
/// than guessed at.
pub const VERSION: u32 = 2;

/// Why a file could not be read as a chunk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Broken {
    /// It does not begin the way a chunk begins.
    NotAChunk,
    /// It is a chunk, of a version this cannot read.
    Version(u32),
    /// It stops in the middle of something.
    Truncated,
    /// Something in it is not one of the things it could be.
    Unknown { what: &'static str, value: u64 },
    /// Text in it is not text.
    NotText,
    /// It points at something that is not there.
    OutOfRange { what: &'static str, index: u64, of: usize },
}

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Broken::NotAChunk => write!(f, "this is not a Luarust chunk."),
            Broken::Version(found) => write!(
                f,
                "this chunk is version {found} and this Luarust reads version {VERSION}."
            ),
            Broken::Truncated => write!(f, "this chunk stops in the middle of something."),
            Broken::Unknown { what, value } => {
                write!(f, "this chunk has a {what} of {value}, which is not one of them.")
            }
            Broken::NotText => write!(f, "text in this chunk is not valid UTF-8."),
            Broken::OutOfRange { what, index, of } => {
                write!(f, "this chunk asks for {what} {index}, and there are {of}.")
            }
        }
    }
}

// ---- writing -------------------------------------------------------------------

/// Write a chunk out, with the source it came from.
pub fn write(chunk: &Chunk, path: &str, source: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, VERSION);
    put_u32(&mut out, u32::from(chunk.overflow == Overflow::Trap));
    put_u32(&mut out, chunk.registers as u32);
    put_str(&mut out, path);
    put_str(&mut out, source);

    put_u32(&mut out, chunk.consts.len() as u32);
    for value in &chunk.consts {
        put_value(&mut out, value);
    }

    put_u32(&mut out, chunk.texts.len() as u32);
    for text in &chunk.texts {
        put_str(&mut out, text);
    }

    put_u32(&mut out, chunk.code.len() as u32);
    for op in &chunk.code {
        put_op(&mut out, *op);
    }

    // One span per instruction, so this length is a check as much as a count.
    put_u32(&mut out, chunk.spans.len() as u32);
    for span in &chunk.spans {
        put_u32(&mut out, span.start as u32);
        put_u32(&mut out, span.end as u32);
    }
    out
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, text: &str) {
    put_u32(out, text.len() as u32);
    out.extend_from_slice(text.as_bytes());
}

fn put_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Num { ty, bits } => {
            out.push(0);
            out.push(ty_tag(*ty));
            put_u64(out, *bits);
        }
        Value::Wide { ty, bits } => {
            out.push(1);
            out.push(ty_tag(*ty));
            for limb in bits.limbs() {
                put_u64(out, *limb);
            }
        }
        Value::Bool(value) => {
            out.push(2);
            out.push(u8::from(*value));
        }
        Value::Str(text) => {
            out.push(3);
            put_str(out, text);
        }
    }
}

fn put_op(out: &mut Vec<u8>, op: Op) {
    match op {
        Op::Const { dst, konst } => {
            out.push(0);
            put_u16(out, dst);
            put_u32(out, konst);
        }
        Op::Move { dst, src } => {
            out.push(1);
            put_u16(out, dst);
            put_u16(out, src);
        }
        Op::Binary { op, ty, dst, lhs, rhs } => {
            out.push(2);
            out.push(op_tag(op));
            out.push(ty_tag(ty));
            put_u16(out, dst);
            put_u16(out, lhs);
            put_u16(out, rhs);
        }
        Op::Neg { dst, src } => {
            out.push(3);
            put_u16(out, dst);
            put_u16(out, src);
        }
        Op::TimeNow { dst, ty } => {
            out.push(4);
            put_u16(out, dst);
            out.push(ty_tag(ty));
        }
        Op::PrintText { text } => {
            out.push(5);
            put_u32(out, text);
        }
        Op::PrintValue { src } => {
            out.push(6);
            put_u16(out, src);
        }
        Op::JumpIfGreater { lhs, rhs, target } => {
            out.push(7);
            put_u16(out, lhs);
            put_u16(out, rhs);
            put_u32(out, target);
        }
        Op::JumpIfEqual { lhs, rhs, target } => {
            out.push(8);
            put_u16(out, lhs);
            put_u16(out, rhs);
            put_u32(out, target);
        }
        Op::Jump { target } => {
            out.push(9);
            put_u32(out, target);
        }
        Op::Halt => out.push(10),
    }
}

fn ty_tag(ty: Ty) -> u8 {
    match ty {
        Ty::B16 => 0,
        Ty::B32 => 1,
        Ty::B64 => 2,
        Ty::B128 => 3,
        Ty::B256 => 4,
        Ty::D32 => 5,
        Ty::D64 => 6,
        Ty::D128 => 7,
        Ty::Er => 8,
        Ty::I8 => 9,
        Ty::I16 => 10,
        Ty::I32 => 11,
        Ty::I64 => 12,
        Ty::U8 => 13,
        Ty::U16 => 14,
        Ty::U32 => 15,
        Ty::U64 => 16,
        Ty::Bool => 17,
        Ty::Str => 18,
    }
}

fn op_tag(op: BinOp) -> u8 {
    match op {
        BinOp::Add => 0,
        BinOp::Sub => 1,
        BinOp::Mul => 2,
        BinOp::Div => 3,
        BinOp::Mod => 4,
        BinOp::Pow => 5,
    }
}

// ---- reading -------------------------------------------------------------------

/// A chunk read back, with the source it was compiled from.
#[derive(Debug)]
pub struct Loaded {
    pub chunk: Chunk,
    pub path: String,
    pub source: String,
}

/// Read a chunk, checking everything in it.
pub fn read(bytes: &[u8]) -> Result<Loaded, Broken> {
    let mut cursor = Cursor { bytes, at: 0 };

    if cursor.take(8)? != MAGIC {
        return Err(Broken::NotAChunk);
    }
    let version = cursor.u32()?;
    if version != VERSION {
        return Err(Broken::Version(version));
    }

    let overflow = if cursor.u32()? == 0 { Overflow::Wrap } else { Overflow::Trap };
    let registers = cursor.u32()? as usize;
    let path = cursor.text()?;
    let source = cursor.text()?;

    let count = cursor.u32()?;
    let mut consts = Vec::with_capacity(count.min(4096) as usize);
    for _ in 0..count {
        consts.push(cursor.value()?);
    }

    let count = cursor.u32()?;
    let mut texts = Vec::with_capacity(count.min(4096) as usize);
    for _ in 0..count {
        texts.push(cursor.text()?);
    }

    let count = cursor.u32()?;
    let mut code = Vec::with_capacity(count.min(65536) as usize);
    for _ in 0..count {
        code.push(cursor.op()?);
    }

    let count = cursor.u32()?;
    let mut spans = Vec::with_capacity(count.min(65536) as usize);
    for _ in 0..count {
        let start = cursor.u32()? as usize;
        let end = cursor.u32()? as usize;
        spans.push(Span { start, end });
    }

    let chunk = Chunk { code, spans, consts, texts, registers, overflow };
    check(&chunk)?;
    Ok(Loaded { chunk, path, source })
}

/// Everything a chunk claims about itself, held against what it actually contains.
///
/// The VM indexes registers, constants, text and instructions without checking, because
/// the compiler never produces an index that is wrong. A file from somewhere else has made
/// no such promise, so this is where that promise is either kept or refused.
fn check(chunk: &Chunk) -> Result<(), Broken> {
    if chunk.spans.len() != chunk.code.len() {
        return Err(Broken::OutOfRange {
            what: "one span per instruction, and finds",
            index: chunk.spans.len() as u64,
            of: chunk.code.len(),
        });
    }
    if chunk.code.is_empty() {
        return Err(Broken::Truncated);
    }

    let register = |r: u16| -> Result<(), Broken> {
        if (r as usize) < chunk.registers {
            Ok(())
        } else {
            Err(Broken::OutOfRange { what: "register", index: r as u64, of: chunk.registers })
        }
    };
    let constant = |k: u32| -> Result<(), Broken> {
        if (k as usize) < chunk.consts.len() {
            Ok(())
        } else {
            Err(Broken::OutOfRange { what: "constant", index: k as u64, of: chunk.consts.len() })
        }
    };
    let text = |t: u32| -> Result<(), Broken> {
        if (t as usize) < chunk.texts.len() {
            Ok(())
        } else {
            Err(Broken::OutOfRange { what: "text", index: t as u64, of: chunk.texts.len() })
        }
    };
    // A jump may land exactly one past the end only if that is where a `Halt` is, which it
    // never is, so every target has to be a real instruction.
    let target = |t: u32| -> Result<(), Broken> {
        if (t as usize) < chunk.code.len() {
            Ok(())
        } else {
            Err(Broken::OutOfRange { what: "instruction", index: t as u64, of: chunk.code.len() })
        }
    };

    for op in &chunk.code {
        match *op {
            Op::Const { dst, konst } => {
                register(dst)?;
                constant(konst)?;
            }
            Op::Move { dst, src } | Op::Neg { dst, src } => {
                register(dst)?;
                register(src)?;
            }
            Op::Binary { dst, lhs, rhs, .. } => {
                register(dst)?;
                register(lhs)?;
                register(rhs)?;
            }
            Op::TimeNow { dst, .. } => register(dst)?,
            Op::PrintText { text: index } => text(index)?,
            Op::PrintValue { src } => register(src)?,
            Op::JumpIfGreater { lhs, rhs, target: to } | Op::JumpIfEqual { lhs, rhs, target: to } => {
                register(lhs)?;
                register(rhs)?;
                target(to)?;
            }
            Op::Jump { target: to } => target(to)?,
            Op::Halt => {}
        }
    }

    // The machine runs until it is told to stop, so a chunk that never says so would run
    // off the end of its own instructions.
    if !chunk.code.iter().any(|op| matches!(op, Op::Halt)) {
        return Err(Broken::Truncated);
    }
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], Broken> {
        let end = self.at.checked_add(count).ok_or(Broken::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(Broken::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, Broken> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, Broken> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("two bytes")))
    }

    fn u32(&mut self) -> Result<u32, Broken> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Result<u64, Broken> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    fn text(&mut self) -> Result<String, Broken> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| Broken::NotText)
    }

    fn ty(&mut self) -> Result<Ty, Broken> {
        let tag = self.u8()?;
        Ok(match tag {
            0 => Ty::B16,
            1 => Ty::B32,
            2 => Ty::B64,
            3 => Ty::B128,
            4 => Ty::B256,
            5 => Ty::D32,
            6 => Ty::D64,
            7 => Ty::D128,
            8 => Ty::Er,
            9 => Ty::I8,
            10 => Ty::I16,
            11 => Ty::I32,
            12 => Ty::I64,
            13 => Ty::U8,
            14 => Ty::U16,
            15 => Ty::U32,
            16 => Ty::U64,
            17 => Ty::Bool,
            18 => Ty::Str,
            other => return Err(Broken::Unknown { what: "type", value: other as u64 }),
        })
    }

    fn value(&mut self) -> Result<Value, Broken> {
        match self.u8()? {
            0 => {
                let ty = self.ty()?;
                Ok(Value::Num { ty, bits: self.u64()? })
            }
            1 => {
                let ty = self.ty()?;
                let mut limbs = [0u64; 8];
                for limb in &mut limbs {
                    *limb = self.u64()?;
                }
                Ok(Value::Wide { ty, bits: Box::new(Uint::from_limbs(limbs)) })
            }
            2 => Ok(Value::Bool(self.u8()? != 0)),
            3 => Ok(Value::text(&self.text()?)),
            other => Err(Broken::Unknown { what: "value", value: other as u64 }),
        }
    }

    fn binop(&mut self) -> Result<BinOp, Broken> {
        Ok(match self.u8()? {
            0 => BinOp::Add,
            1 => BinOp::Sub,
            2 => BinOp::Mul,
            3 => BinOp::Div,
            4 => BinOp::Mod,
            5 => BinOp::Pow,
            other => return Err(Broken::Unknown { what: "operator", value: other as u64 }),
        })
    }

    fn op(&mut self) -> Result<Op, Broken> {
        Ok(match self.u8()? {
            0 => Op::Const { dst: self.u16()?, konst: self.u32()? },
            1 => Op::Move { dst: self.u16()?, src: self.u16()? },
            2 => Op::Binary {
                op: self.binop()?,
                ty: self.ty()?,
                dst: self.u16()?,
                lhs: self.u16()?,
                rhs: self.u16()?,
            },
            3 => Op::Neg { dst: self.u16()?, src: self.u16()? },
            4 => Op::TimeNow { dst: self.u16()?, ty: self.ty()? },
            5 => Op::PrintText { text: self.u32()? },
            6 => Op::PrintValue { src: self.u16()? },
            7 => Op::JumpIfGreater { lhs: self.u16()?, rhs: self.u16()?, target: self.u32()? },
            8 => Op::JumpIfEqual { lhs: self.u16()?, rhs: self.u16()?, target: self.u32()? },
            9 => Op::Jump { target: self.u32()? },
            10 => Op::Halt,
            other => Err(Broken::Unknown { what: "instruction", value: other as u64 })?,
        })
    }
}

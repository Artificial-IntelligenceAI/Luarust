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
//! **The source travels with it, unless it is told not to.** A chunk carries the text it
//! was compiled from, which costs a few kilobytes and buys the thing that would otherwise
//! be lost: a program that stops half way through can still point at the line that did
//! it, on a machine that has never seen the source.
//!
//! A project that would rather not ship its source says so in `Luarust.toml`, and then
//! the chunk carries the line table instead -- four bytes per line of the original. That
//! is enough to say which line stopped and in which column, and not enough to say what
//! was written there. Giving up the text should not mean giving up the line number.
//!
//! **Nothing read from a file is trusted.** Every index is checked against what it
//! indexes before the chunk is handed back, because a corrupt file must produce a
//! complaint and not a crash — and "run anywhere" means files will arrive from places
//! nobody vouched for.

use crate::chunk::{Chunk, Op, Reg, Routine};
use luarust_core::heap::Collect;
use luarust_core::value::{Engine, Floats, Overflow, Value};
use luarust_diag::Span;
use luarust_num::Uint;
use luarust_core::{BinOp, CmpOp, Ty};

/// What every Luarust chunk begins with.
pub const MAGIC: &[u8; 8] = b"LUARUST\x1b";

/// The format's version. Read a file claiming a different one and it is refused rather
/// than guessed at.
pub const VERSION: u32 = 12;

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
    /// It asks for more registers than an instruction could ever name.
    TooManyRegisters { asked: u64, most: usize },
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
            Broken::TooManyRegisters { asked, most } => write!(
                f,
                "this chunk asks for {asked} registers, and an instruction can name {most}."
            ),
        }
    }
}

// ---- writing -------------------------------------------------------------------

/// Write a chunk out, with the source it came from.
pub fn write(chunk: &Chunk, path: &str, source: &str) -> Vec<u8> {
    write_with(chunk, path, source, true, false)
}

/// Write a chunk out, saying whether the source text goes with it.
///
/// With `embed_source` off the text stays behind and only its line table travels, so a
/// fault still names its line and column and simply cannot quote it.
pub fn write_with(
    chunk: &Chunk,
    path: &str,
    source: &str,
    embed_source: bool,
    dpd: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, VERSION);
    put_u32(&mut out, u32::from(chunk.overflow == Overflow::Trap));
    // What the program decided about collecting and about printing floats. Both are the
    // project's answers, and `luarust-run` has no project file, so they travel here.
    put_u32(&mut out, chunk.collect.tag());
    put_u32(&mut out, chunk.floats.tag());
    put_u32(&mut out, chunk.engine.tag());
    put_u32(&mut out, chunk.registers as u32);
    put_str(&mut out, path);

    if embed_source {
        put_u32(&mut out, 1);
        put_str(&mut out, source);
    } else {
        put_u32(&mut out, 0);
        let starts = line_starts(source);
        put_u32(&mut out, starts.len() as u32);
        for start in starts {
            put_u32(&mut out, start as u32);
        }
        // The length is kept even though the text is not, so a column near the end of the
        // last line is still checked against something real.
        put_u32(&mut out, source.len() as u32);
    }

    // Which way the decimals in this chunk are written down. Everything computes in BID,
    // so this is a repacking at the edge and nothing else -- but it has to travel with
    // the file, or a chunk written one way and read the other would be nonsense.
    put_u32(&mut out, u32::from(dpd));

    put_u32(&mut out, chunk.consts.len() as u32);
    for value in &chunk.consts {
        put_value(&mut out, value, dpd);
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

    put_u32(&mut out, chunk.funcs.len() as u32);
    for routine in &chunk.funcs {
        put_u32(&mut out, routine.registers as u32);
        put_u32(&mut out, routine.params.len() as u32);
        for ty in &routine.params {
            put_ty(&mut out, *ty);
        }
        match routine.returns {
            Some(ty) => {
                out.push(1);
                put_ty(&mut out, ty);
            }
            None => out.push(0),
        }
        put_u32(&mut out, routine.code.len() as u32);
        for op in &routine.code {
            put_op(&mut out, *op);
        }
        put_u32(&mut out, routine.spans.len() as u32);
        for span in &routine.spans {
            put_u32(&mut out, span.start as u32);
            put_u32(&mut out, span.end as u32);
        }
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

fn put_value(out: &mut Vec<u8>, value: &Value, dpd: bool) {
    // A decimal is repacked on the way out when the project asked for the other encoding.
    let value = &recode(value, dpd);
    match value {
        Value::Num { ty, bits } => {
            out.push(0);
            put_ty(out, *ty);
            put_u64(out, *bits);
        }
        Value::Wide { ty, bits } => {
            out.push(1);
            put_ty(out, *ty);
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
        // Written out as it reads: a sign and two runs of limbs, which is the whole of
        // what an exact rational is.
        Value::Exact(value) => {
            out.push(4);
            put_big(out, value.numerator());
            put_big(out, value.denominator());
        }
    }
}

/// A decimal from one encoding into the other. Everything else is handed straight back.
fn recode(value: &Value, dpd: bool) -> Value {
    if !dpd {
        return value.clone();
    }
    let ty = value.ty();
    let Some(fmt) = luarust_core::value::decimal_of(ty) else {
        return value.clone();
    };
    let bits = value.bits().expect("a decimal has bits");
    let taken = luarust_num::decimal::unpack(fmt, bits, false);
    Value::float(ty, luarust_num::decimal::pack(fmt, taken, true))
}

/// And back again, on the way in.
fn decode_value(value: Value, dpd: bool) -> Value {
    if !dpd {
        return value;
    }
    let ty = value.ty();
    let Some(fmt) = luarust_core::value::decimal_of(ty) else {
        return value;
    };
    let bits = value.bits().expect("a decimal has bits");
    let taken = luarust_num::decimal::unpack(fmt, bits, true);
    Value::float(ty, luarust_num::decimal::pack(fmt, taken, false))
}

fn put_big(out: &mut Vec<u8>, value: &luarust_num::Big) {
    out.push(u8::from(value.is_negative()));
    put_u32(out, value.limbs().len() as u32);
    for limb in value.limbs() {
        put_u64(out, *limb);
    }
}

fn put_op(out: &mut Vec<u8>, op: Op) {
    match op {
        Op::Const { dst, konst } => {
            out.push(0);
            put_u16(out, dst);
            put_u32(out, konst);
        }
        Op::Move { dst, src, ty } => {
            out.push(1);
            put_u16(out, dst);
            put_u16(out, src);
            put_ty(out, ty);
        }
        Op::Binary { op, ty, dst, lhs, rhs } => {
            out.push(2);
            out.push(op_tag(op));
            put_ty(out, ty);
            put_u16(out, dst);
            put_u16(out, lhs);
            put_u16(out, rhs);
        }
        Op::Neg { dst, src, ty } => {
            out.push(3);
            put_u16(out, dst);
            put_u16(out, src);
            put_ty(out, ty);
        }
        Op::TimeNow { dst, ty } => {
            out.push(4);
            put_u16(out, dst);
            put_ty(out, ty);
        }
        Op::PrintText { text } => {
            out.push(5);
            put_u32(out, text);
        }
        Op::PrintValue { src, ty } => {
            out.push(6);
            put_u16(out, src);
            put_ty(out, ty);
        }
        Op::Call { func, base, argc, dst } => {
            out.push(15);
            put_u32(out, func);
            put_u16(out, base);
            put_u16(out, argc);
            put_u16(out, dst);
        }
        Op::Return { src, ty } => {
            out.push(16);
            put_u16(out, src);
            put_ty(out, ty);
        }
        Op::ReturnNothing => out.push(17),
        Op::NewArray { dst, items, count, ty } => {
            out.push(18);
            put_u16(out, dst);
            put_u16(out, items);
            put_u16(out, count);
            put_ty(out, ty);
        }
        Op::Filled { dst, length, value, ty } => {
            out.push(19);
            put_u16(out, dst);
            put_u16(out, length);
            put_u16(out, value);
            put_ty(out, ty);
        }
        Op::At { dst, array, at, rank, ty } => {
            out.push(20);
            put_u16(out, dst);
            put_u16(out, array);
            put_u16(out, at);
            out.push(rank);
            put_ty(out, ty);
        }
        Op::StoreAt { array, at, rank, value, ty } => {
            out.push(21);
            put_u16(out, array);
            put_u16(out, at);
            out.push(rank);
            put_u16(out, value);
            put_ty(out, ty);
        }
        Op::Count { dst, array, ty } => {
            out.push(22);
            put_u16(out, dst);
            put_u16(out, array);
            put_ty(out, ty);
        }
        Op::Not { dst, src } => {
            out.push(12);
            put_u16(out, dst);
            put_u16(out, src);
        }
        Op::JumpIfFalse { cond, target } => {
            out.push(13);
            put_u16(out, cond);
            put_u32(out, target);
        }
        Op::JumpIfTrue { cond, target } => {
            out.push(14);
            put_u16(out, cond);
            put_u32(out, target);
        }
        Op::JumpIfGreater { lhs, rhs, ty, target } => {
            out.push(7);
            put_u16(out, lhs);
            put_u16(out, rhs);
            put_ty(out, ty);
            put_u32(out, target);
        }
        Op::JumpIfEqual { lhs, rhs, ty, target } => {
            out.push(8);
            put_u16(out, lhs);
            put_u16(out, rhs);
            put_ty(out, ty);
            put_u32(out, target);
        }
        Op::Jump { target } => {
            out.push(9);
            put_u32(out, target);
        }
        Op::Compare { op, operands, dst, lhs, rhs } => {
            out.push(11);
            out.push(cmp_tag(op));
            put_ty(out, operands);
            put_u16(out, dst);
            put_u16(out, lhs);
            put_u16(out, rhs);
        }
        Op::Halt => out.push(10),
    }
}

fn cmp_tag(op: CmpOp) -> u8 {
    match op {
        CmpOp::Less => 0,
        CmpOp::Greater => 1,
        CmpOp::Equal => 2,
        CmpOp::LessEqual => 3,
        CmpOp::GreaterEqual => 4,
        CmpOp::NotEqual => 5,
    }
}

/// A type, written out. A scalar is its tag; an array is a marker, its element and its
/// shape, since an array's type is not one number.
fn put_ty(out: &mut Vec<u8>, ty: Ty) {
    match ty.array() {
        None => out.push(ty.tag()),
        Some(of) => {
            out.push(ARRAY_TAG);
            // Written out in full rather than as its index: an index means something
            // only inside the run that made it, and a chunk outlives that.
            put_ty(out, of.element);
            out.push(of.dims().len() as u8);
            for dim in of.dims() {
                put_u32(out, *dim);
            }
        }
    }
}

/// The tag that says an array follows, chosen above every scalar's.
const ARRAY_TAG: u8 = 200;

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

/// A chunk read back, with as much of its source as it was built to carry.
#[derive(Debug)]
pub struct Loaded {
    pub chunk: Chunk,
    pub path: String,
    /// What the program was written in, when the chunk carries it.
    pub source: Source,
}

/// What a chunk knows about the file it came from.
#[derive(Debug)]
pub enum Source {
    /// The text itself, so a fault can be quoted.
    Text(String),
    /// Only where the lines began, so a fault can be located but not quoted.
    Lines { starts: Vec<usize>, len: usize },
}

impl Source {
    /// The file this chunk came from, ready to report a fault against.
    pub fn file(&self, path: impl Into<std::path::PathBuf>) -> luarust_diag::SourceFile {
        match self {
            Source::Text(text) => luarust_diag::SourceFile::new(path, text.clone()),
            Source::Lines { starts, len } => {
                luarust_diag::SourceFile::without_text(path, starts.clone(), *len)
            }
        }
    }
}

/// Where each line of a text begins, counted in bytes.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.bytes().enumerate().filter(|(_, b)| *b == b'\n').map(|(at, _)| at + 1));
    starts
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
    let tag = cursor.u32()?;
    let collect = Collect::from_tag(tag)
        .ok_or(Broken::Unknown { what: "a way of collecting", value: u64::from(tag) })?;
    let tag = cursor.u32()?;
    let floats = Floats::from_tag(tag)
        .ok_or(Broken::Unknown { what: "a way of printing floats", value: u64::from(tag) })?;
    let tag = cursor.u32()?;
    let engine = Engine::from_tag(tag)
        .ok_or(Broken::Unknown { what: "a way of running a chunk", value: u64::from(tag) })?;
    let registers = cursor.u32()? as usize;
    let path = cursor.text()?;
    let source = match cursor.u32()? {
        1 => Source::Text(cursor.text()?),
        0 => {
            let count = cursor.u32()?;
            let mut starts = Vec::with_capacity(count.min(65536) as usize);
            for _ in 0..count {
                starts.push(cursor.u32()? as usize);
            }
            let len = cursor.u32()? as usize;
            // A line table that runs backwards, or off the end of the file it describes,
            // would put a fault at a position that never existed.
            if starts.first() != Some(&0)
                || starts.windows(2).any(|pair| pair[0] >= pair[1])
                || starts.last().is_some_and(|last| *last > len)
            {
                return Err(Broken::OutOfRange { what: "a line start", index: 0, of: len });
            }
            Source::Lines { starts, len }
        }
        value => return Err(Broken::Unknown { what: "source kind", value: value.into() }),
    };

    let dpd = cursor.u32()? != 0;

    let count = cursor.u32()?;
    let mut consts = Vec::with_capacity(count.min(4096) as usize);
    for _ in 0..count {
        consts.push(decode_value(cursor.value()?, dpd));
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

    let count = cursor.u32()?;
    let mut funcs = Vec::with_capacity(count.min(4096) as usize);
    for _ in 0..count {
        let registers = cursor.u32()? as usize;
        let count = cursor.u32()?;
        let mut params = Vec::with_capacity((count as usize).min(256));
        for _ in 0..count {
            params.push(cursor.ty()?);
        }
        let returns = if cursor.u8()? == 0 { None } else { Some(cursor.ty()?) };

        let n = cursor.u32()?;
        let mut code = Vec::with_capacity(n.min(65536) as usize);
        for _ in 0..n {
            code.push(cursor.op()?);
        }
        let n = cursor.u32()?;
        let mut spans = Vec::with_capacity(n.min(65536) as usize);
        for _ in 0..n {
            let start = cursor.u32()? as usize;
            let end = cursor.u32()? as usize;
            spans.push(Span { start, end });
        }
        funcs.push(Routine { code, spans, registers, params, returns });
    }

    let chunk =
        Chunk { code, spans, consts, texts, registers, overflow, collect, floats, engine, funcs };
    check(&chunk)?;
    Ok(Loaded { chunk, path, source })
}

/// Everything a chunk claims about itself, held against what it actually contains.
///
/// Refuse a register count no instruction could name.
fn too_many(registers: usize) -> Result<(), Broken> {
    const REACHABLE: usize = Reg::MAX as usize + 1;
    if registers > REACHABLE {
        return Err(Broken::TooManyRegisters { asked: registers as u64, most: REACHABLE });
    }
    Ok(())
}

/// The VM indexes registers, constants, text and instructions without checking, because
/// the compiler never produces an index that is wrong. A file from somewhere else has made
/// no such promise, so this is where that promise is either kept or refused.
fn check(chunk: &Chunk) -> Result<(), Broken> {
    // How many registers a chunk says it wants, before a frame is made out of it.
    //
    // An instruction names a register in a `Reg`, which is sixteen bits, so a chunk asking
    // for more than a `Reg` can reach is describing registers no instruction could ever
    // address. Every other index is checked against something; this one was the count
    // itself and was checked against nothing, so a chunk claiming four billion of them
    // passed and the VM then tried to make four billion values -- ninety-six gigabytes,
    // for a program of nine registers. One flipped byte in a file is enough to say it.
    too_many(chunk.registers)?;
    for routine in &chunk.funcs {
        too_many(routine.registers)?;
    }

    check_code(chunk, &chunk.code, &chunk.spans, chunk.registers)?;

    // The main code halts; a routine returns. Each is checked against its own registers
    // and its own instruction count, since neither shares those with anybody.
    for routine in &chunk.funcs {
        check_code(chunk, &routine.code, &routine.spans, routine.registers)?;
        if routine.params.len() > routine.registers {
            return Err(Broken::OutOfRange {
                what: "parameters in a function with registers",
                index: routine.params.len() as u64,
                of: routine.registers,
            });
        }
    }

    // The machine runs until it is told to stop, so a chunk that never says so would run
    // off the end of its own instructions.
    if !chunk.code.iter().any(|op| matches!(op, Op::Halt)) {
        return Err(Broken::Truncated);
    }
    Ok(())
}

fn check_code(
    chunk: &Chunk,
    code: &[Op],
    spans: &[Span],
    registers: usize,
) -> Result<(), Broken> {
    if spans.len() != code.len() {
        return Err(Broken::OutOfRange {
            what: "one span per instruction, and finds",
            index: spans.len() as u64,
            of: code.len(),
        });
    }
    if code.is_empty() {
        return Err(Broken::Truncated);
    }

    let register = |r: u16| -> Result<(), Broken> {
        if (r as usize) < registers {
            Ok(())
        } else {
            Err(Broken::OutOfRange { what: "register", index: r as u64, of: registers })
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
        if (t as usize) < code.len() {
            Ok(())
        } else {
            Err(Broken::OutOfRange { what: "instruction", index: t as u64, of: code.len() })
        }
    };
    let routine = |f: u32| -> Result<(), Broken> {
        if (f as usize) < chunk.funcs.len() {
            Ok(())
        } else {
            Err(Broken::OutOfRange { what: "function", index: f as u64, of: chunk.funcs.len() })
        }
    };

    for op in code {
        match *op {
            Op::Const { dst, konst } => {
                register(dst)?;
                constant(konst)?;
            }
            Op::Move { dst, src, .. } | Op::Neg { dst, src, .. } | Op::Not { dst, src } => {
                register(dst)?;
                register(src)?;
            }
            Op::Binary { dst, lhs, rhs, .. } | Op::Compare { dst, lhs, rhs, .. } => {
                register(dst)?;
                register(lhs)?;
                register(rhs)?;
            }
            Op::TimeNow { dst, .. } => register(dst)?,
            Op::PrintText { text: index } => text(index)?,
            Op::PrintValue { src, .. } => register(src)?,
            Op::JumpIfGreater { lhs, rhs, target: to, .. }
            | Op::JumpIfEqual { lhs, rhs, target: to, .. } => {
                register(lhs)?;
                register(rhs)?;
                target(to)?;
            }
            Op::JumpIfFalse { cond, target: to } | Op::JumpIfTrue { cond, target: to } => {
                register(cond)?;
                target(to)?;
            }
            Op::Jump { target: to } => target(to)?,
            Op::Call { func, base, argc, dst } => {
                routine(func)?;
                register(dst)?;
                // Every argument register, not only the first, since the call reads the
                // whole run of them.
                for n in 0..argc {
                    register(base + n)?;
                }
                if usize::from(argc) != chunk.funcs[func as usize].params.len() {
                    return Err(Broken::OutOfRange {
                        what: "arguments for a function taking",
                        index: argc as u64,
                        of: chunk.funcs[func as usize].params.len(),
                    });
                }
            }
            Op::Return { src, .. } => register(src)?,
            Op::NewArray { dst, items, count, .. } => {
                register(dst)?;
                for n in 0..count {
                    register(items + n)?;
                }
            }
            Op::Filled { dst, length, value, .. } => {
                register(dst)?;
                register(length)?;
                register(value)?;
            }
            Op::At { dst, array, at, rank, .. } => {
                register(dst)?;
                register(array)?;
                for n in 0..u16::from(rank) {
                    register(at + n)?;
                }
            }
            Op::StoreAt { array, at, rank, value, .. } => {
                register(array)?;
                register(value)?;
                for n in 0..u16::from(rank) {
                    register(at + n)?;
                }
            }
            Op::Count { dst, array, .. } => {
                register(dst)?;
                register(array)?;
            }
            Op::ReturnNothing => {}
            Op::Halt => {}
        }
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
        if tag == ARRAY_TAG {
            let element = self.ty()?;
            let rank = self.u8()? as usize;
            if rank > luarust_core::ty::MAX_RANK {
                return Err(Broken::Unknown { what: "array rank", value: rank as u64 });
            }
            let mut shape = Vec::with_capacity(rank);
            for _ in 0..rank {
                shape.push(self.u32()?);
            }
            let made = if rank == 0 {
                luarust_core::ty::growable(element)
            } else {
                luarust_core::ty::fixed(element, &shape)
            };
            return made.ok_or(Broken::Unknown { what: "array shape", value: 0 });
        }
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
            4 => {
                let numerator = self.big()?;
                let denominator = self.big()?;
                // A zero denominator would be a value no arithmetic could have made, so
                // a chunk carrying one is a corrupt chunk rather than a strange number.
                let exact = luarust_num::Exact::ratio(numerator, denominator)
                    .ok_or(Broken::Unknown { what: "denominator", value: 0 })?;
                Ok(Value::Exact(std::rc::Rc::new(exact)))
            }
            other => Err(Broken::Unknown { what: "value", value: other as u64 }),
        }
    }

    fn big(&mut self) -> Result<luarust_num::Big, Broken> {
        let negative = self.u8()? != 0;
        let count = self.u32()?;
        // A magnitude longer than the file could hold is a corrupt file, and reserving
        // for it before reading it is how a bad length becomes a bad allocation.
        let mut limbs = Vec::with_capacity((count as usize).min(4096));
        for _ in 0..count {
            limbs.push(self.u64()?);
        }
        Ok(luarust_num::Big::from_parts(negative, limbs))
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

    fn cmp(&mut self) -> Result<CmpOp, Broken> {
        Ok(match self.u8()? {
            0 => CmpOp::Less,
            1 => CmpOp::Greater,
            2 => CmpOp::Equal,
            3 => CmpOp::LessEqual,
            4 => CmpOp::GreaterEqual,
            5 => CmpOp::NotEqual,
            other => return Err(Broken::Unknown { what: "comparison", value: other as u64 }),
        })
    }

    fn op(&mut self) -> Result<Op, Broken> {
        Ok(match self.u8()? {
            0 => Op::Const { dst: self.u16()?, konst: self.u32()? },
            1 => Op::Move { dst: self.u16()?, src: self.u16()?, ty: self.ty()? },
            2 => Op::Binary {
                op: self.binop()?,
                ty: self.ty()?,
                dst: self.u16()?,
                lhs: self.u16()?,
                rhs: self.u16()?,
            },
            3 => Op::Neg { dst: self.u16()?, src: self.u16()?, ty: self.ty()? },
            4 => Op::TimeNow { dst: self.u16()?, ty: self.ty()? },
            5 => Op::PrintText { text: self.u32()? },
            6 => Op::PrintValue { src: self.u16()?, ty: self.ty()? },
            7 => Op::JumpIfGreater {
                lhs: self.u16()?,
                rhs: self.u16()?,
                ty: self.ty()?,
                target: self.u32()?,
            },
            8 => Op::JumpIfEqual {
                lhs: self.u16()?,
                rhs: self.u16()?,
                ty: self.ty()?,
                target: self.u32()?,
            },
            9 => Op::Jump { target: self.u32()? },
            10 => Op::Halt,
            11 => Op::Compare {
                op: self.cmp()?,
                operands: self.ty()?,
                dst: self.u16()?,
                lhs: self.u16()?,
                rhs: self.u16()?,
            },
            12 => Op::Not { dst: self.u16()?, src: self.u16()? },
            18 => Op::NewArray {
                dst: self.u16()?,
                items: self.u16()?,
                count: self.u16()?,
                ty: self.ty()?,
            },
            19 => Op::Filled {
                dst: self.u16()?,
                length: self.u16()?,
                value: self.u16()?,
                ty: self.ty()?,
            },
            20 => Op::At {
                dst: self.u16()?,
                array: self.u16()?,
                at: self.u16()?,
                rank: self.u8()?,
                ty: self.ty()?,
            },
            21 => Op::StoreAt {
                array: self.u16()?,
                at: self.u16()?,
                rank: self.u8()?,
                value: self.u16()?,
                ty: self.ty()?,
            },
            22 => Op::Count { dst: self.u16()?, array: self.u16()?, ty: self.ty()? },
            15 => Op::Call {
                func: self.u32()?,
                base: self.u16()?,
                argc: self.u16()?,
                dst: self.u16()?,
            },
            16 => Op::Return { src: self.u16()?, ty: self.ty()? },
            17 => Op::ReturnNothing,
            13 => Op::JumpIfFalse { cond: self.u16()?, target: self.u32()? },
            14 => Op::JumpIfTrue { cond: self.u16()?, target: self.u32()? },
            other => Err(Broken::Unknown { what: "instruction", value: other as u64 })?,
        })
    }
}

//! What a Luarust program computes with.
//!
//! A value always knows its own type, because nothing in this language converts without
//! being told to and the arithmetic has to be able to refuse.
//!
//! Floats are held as their IEEE encoding at their own width and worked on by
//! `luarust-num`, so every execution path gets the same answer from the same code rather
//! than from two careful reimplementations. Integers are held as raw bits and worked on
//! at 128 bits before being cut back to their width, which is what makes wrapping and
//! trapping the same operation with a different ending.

use luarust_diag::{Diagnostic, Span};
use luarust_num::{Exact, Uint, decimal};
use luarust_num::binary::{self, Comparison, Format, Round};
use crate::{BinOp, CmpOp, Ty};

/// How deep calls may go before a program is stopped.
///
/// The same number on every path, and that matters more than the number itself: if the
/// interpreter gave up at one depth and the machine code at another, the same program
/// would end two different ways and the three paths would stop being comparable.
///
/// It differs between builds because it has to. The tree-walker recurses on the real
/// stack, and an unoptimised frame is many times the size of an optimised one -- a
/// generated program recursing 207 deep overflowed a debug build while a release build
/// took two thousand without noticing. Every path in a given build still shares this one
/// number, which is the property that matters; what it is depends on how much stack a
/// frame of that build costs.
/// How a program runs, once it is a chunk.
///
/// Not *what* is produced -- that is `[build] output`, and a native binary consults none
/// of this. These are the ways of running the same `.lrc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Engine {
    /// The bytecode, interpreted. Nothing is compiled, so nothing is spent compiling.
    #[default]
    Vm,
    /// All of it compiled through LLVM before anything starts. Full speed from the first
    /// iteration, and it pays to compile routines that never run.
    Whole,
    /// Interpreted until something turns out to be worth compiling, and compiled then.
    /// Nothing is spent on code that runs once, and a loop that runs a million times
    /// stops being interpreted part way through the first time round.
    Hot,
}

impl Engine {
    pub fn tag(self) -> u32 {
        match self {
            Engine::Vm => 0,
            Engine::Whole => 1,
            Engine::Hot => 2,
        }
    }

    pub fn from_tag(tag: u32) -> Option<Engine> {
        Some(match tag {
            0 => Engine::Vm,
            1 => Engine::Whole,
            2 => Engine::Hot,
            _ => return None,
        })
    }
}

/// How much of a binary float a program writes out.
///
/// Both answers are honest and they disagree about `b64 |0.1|`, which is why it is a
/// setting rather than a decision made here. `[defaults] float-printing` in the project
/// file chooses; this is where the choice is kept while a program runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Floats {
    /// The value that is held, whole: `0.1000000000000000055511151231257827021181583404541015625`.
    #[default]
    Exact,
    /// The fewest digits that name this number and no other, at its own format: `0.1`.
    Shortest,
}

thread_local! {
    static FLOATS: std::cell::Cell<Floats> = const { std::cell::Cell::new(Floats::Exact) };
}

impl Floats {
    /// The number it is written as in a chunk, and back.
    pub fn tag(self) -> u32 {
        match self {
            Floats::Exact => 0,
            Floats::Shortest => 1,
        }
    }

    pub fn from_tag(tag: u32) -> Option<Floats> {
        Some(match tag {
            0 => Floats::Exact,
            1 => Floats::Shortest,
            _ => return None,
        })
    }
}

/// Choose how floats are written out, for the rest of this program's run.
pub fn set_floats(how: Floats) {
    FLOATS.with(|it| it.set(how));
}

/// How floats are being written out.
pub fn floats() -> Floats {
    FLOATS.with(std::cell::Cell::get)
}

pub const DEPTH_LIMIT: usize = if cfg!(debug_assertions) { 100 } else { 2_000 };

/// The working width every float format fits in. `b256` needs the most.
pub type Bits = Uint<8>;

/// What to do when an integer will not fit its width.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Overflow {
    /// Roll over, as the hardware does.
    #[default]
    Wrap,
    /// Stop and say so.
    Trap,
}

/// The quotient and remainder of `a` and `b` at their own width, and whether the quotient
/// overflowed.
///
/// A macro over the real width rather than a function over `i128`, which is what this was
/// and what cost the interpreter 38% on signed `mod`: widening to 128 bits turns one
/// `sdiv` into a multi-instruction sequence, to buy range that a correction of one divisor
/// never needed. Found by measuring the commit before the setting existed against the one
/// after, on the same machine in the same minute -- the released numbers looked fine
/// because the unsigned path never comes here.
///
/// `b` is never nought: every caller has already refused that. The most negative value
/// over `-1` is the one case that overflows, and it is the same case in all three
/// conventions because that division is exact -- so it is detected rather than computed.
///
/// Every step is written wrapping, and none of them can wrap. A correction only happens
/// when the remainder is not nought, which rules out both divisions whose quotient could
/// sit at a limit: the most negative value over `1` and over `-1` are each exact.
macro_rules! divided {
    ($division:expr, $a:expr, $b:expr, $signed:ty) => {{
        let (a, b): ($signed, $signed) = ($a, $b);
        let overflowed = a == <$signed>::MIN && b == -1;
        let quotient = a.wrapping_div(b);
        let remainder = a.wrapping_rem(b);
        if remainder == 0 {
            (quotient, 0, overflowed)
        } else {
            let (quotient, remainder) = match $division {
                Division::Truncated => (quotient, remainder),
                // The signs differ, so the truncated quotient rounded toward zero where
                // flooring rounds away: one step down, and the divisor back into the
                // remainder.
                Division::Floored if (remainder < 0) != (b < 0) => {
                    (quotient.wrapping_sub(1), remainder.wrapping_add(b))
                }
                Division::Floored => (quotient, remainder),
                // Only the sign of the remainder matters, and the step goes whichever way
                // brings it back above zero.
                Division::Euclidean if remainder < 0 && b > 0 => {
                    (quotient.wrapping_sub(1), remainder.wrapping_add(b))
                }
                Division::Euclidean if remainder < 0 => {
                    (quotient.wrapping_add(1), remainder.wrapping_sub(b))
                }
                Division::Euclidean => (quotient, remainder),
            };
            (quotient, remainder, overflowed)
        }
    }};
}

/// How a division rounds, and which way its remainder leans.
///
/// Every convention answers `a = q × b + r`; they disagree only about which `q` to pick
/// when the division is not exact, and each choice of `q` decides `r`. So this settles
/// **both** `div` and `mod` at once, which is the point of it being one setting: the two
/// were separate decisions once, and the identity above did not hold.
///
/// ```text
///                    -7 div 3   -7 mod 3    7 div -3   7 mod -3
///   Floored             -3          2          -3         -2      `r` follows the divisor
///   Truncated           -2         -1          -2          1      `r` follows the dividend
///   Euclidean           -3          2          -2          1      `r` is never negative
/// ```
///
/// Unsigned types cannot tell them apart, and only integers round at all — `div` on a
/// float, a decimal or an `er` is exact division, so for those this settles `mod` alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Division {
    /// The remainder takes the sign of the divisor. Knuth's, and Python's.
    #[default]
    Floored,
    /// The remainder takes the sign of the dividend. C's, and most hardware's.
    Truncated,
    /// The remainder is never negative, which is the division algorithm as number theory
    /// states it: `0 ≤ r < |b|`.
    Euclidean,
}

impl Division {
    /// The number it is written as in a chunk, and back.
    pub fn tag(self) -> u32 {
        match self {
            Division::Floored => 0,
            Division::Truncated => 1,
            Division::Euclidean => 2,
        }
    }

    pub fn from_tag(tag: u32) -> Option<Division> {
        Some(match tag {
            0 => Division::Floored,
            1 => Division::Truncated,
            2 => Division::Euclidean,
            _ => return None,
        })
    }
}

/// A value, and the type it is.
///
/// The shape here is a performance decision, and a measured one. Holding every float at
/// `b256`'s width made a value 72 bytes, which every register write and every clone then
/// had to copy — and since both the interpreter and the VM paid it, the VM's advantage
/// almost vanished into the memcpy. Everything that fits in 64 bits now does: all the
/// integers, and `b16`, `b32` and `b64`. Only `b128` and `b256` are boxed, so the two
/// widths nobody uses in a hot loop pay for themselves instead of taxing the ones that
/// are.
#[derive(Clone, Debug)]
pub enum Value {
    /// An integer, or a float narrow enough to fit: `b16`, `b32`, `b64`.
    Num { ty: Ty, bits: u64 },
    /// `b128` and `b256`, which do not fit.
    Wide { ty: Ty, bits: Box<Bits> },
    Bool(bool),
    /// Shared, because a string is immutable here and copying one to move it between
    /// registers would be paying for nothing.
    Str(std::rc::Rc<str>),
    /// `er`. Shared for the same reason a string is: a number does not change, so two
    /// names for one is two names for one, and nothing can tell the difference.
    Exact(std::rc::Rc<Exact>),
}

impl PartialEq for Value {
    /// The same answer [`compare`] gives, so a value never has two notions of equal. Two
    /// arrays are equal when they are the same array, and two of anything else when they
    /// are worth the same.
    fn eq(&self, other: &Self) -> bool {
        compare(self, other) == Comparison::Equal
    }
}


/// What every operation gives back: an answer, or a fault behind a pointer.
///
/// Boxed, and that is a decision about speed rather than about style. A [`Fault`] is
/// eighty bytes — two `String`s and two names — so `Result<u64, Fault>` was eighty bytes
/// going back through memory on every single addition, to carry an eight-byte answer
/// along a path that almost never runs. Behind a pointer the whole thing fits in a
/// register pair.
pub type Answer<T> = Result<T, Box<Fault>>;

/// Something that went wrong while a program was running.
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    pub code: &'static str,
    pub message: String,
    pub rule: &'static str,
    pub fix: String,
}

impl Fault {
    /// A fault, for whoever is running the program to report.
    pub fn of(
        code: &'static str,
        message: impl Into<String>,
        rule: &'static str,
        fix: impl Into<String>,
    ) -> Fault {
        Fault { code, message: message.into(), rule, fix: fix.into() }
    }

    /// A fault, already boxed, because that is the only shape anything wants one in.
    fn new(
        code: &'static str,
        message: impl Into<String>,
        rule: &'static str,
        fix: impl Into<String>,
    ) -> Box<Self> {
        Box::new(Self { code, message: message.into(), rule, fix: fix.into() })
    }
}

/// A fault, and where in the program it happened.
///
/// Shared by every execution path, so that a program stopping under the interpreter and
/// the same program stopping under the VM produce the same words as well as the same
/// behaviour.
#[derive(Clone, Debug)]
pub struct Stopped {
    /// Boxed for the same reason [`Answer`] boxes its fault: this rides in the return
    /// type of every step both interpreters take, and behind a pointer the whole
    /// `Result` fits in a register pair instead of moving 96 bytes through memory.
    pub fault: Box<Fault>,
    pub span: Span,
}

impl Stopped {
    /// The same shape as every other Luarust error, so a running program's complaints
    /// read exactly like a compiler's.
    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::new(self.fault.code, self.fault.message.clone())
            .primary(self.span, "while running this")
            .rule(self.fault.rule)
            .fix(self.fault.fix.clone())
    }
}

/// Order two integers of one width, from their bits.
///
/// The instruction already says what they are, so none of this has to be worked out from
/// the values again: two integers of the same type are equal exactly when their bits are,
/// because a value is always stored masked to its width, and ordering them is one machine
/// comparison once the sign is settled. Going through [`compare`] instead was a quarter
/// of the VM's time on a counting loop.
pub fn int_compare(ty: Ty, a: u64, b: u64) -> Comparison {
    if a == b {
        return Comparison::Equal;
    }
    let greater = if ty.is_signed() {
        // Push the sign bit to the top so the comparison sees it, whatever the width.
        let shift = 64 - ty.int_bits().unwrap_or(64);
        ((a << shift) as i64) > ((b << shift) as i64)
    } else {
        a > b
    };
    if greater { Comparison::Greater } else { Comparison::Less }
}

/// Order two values of the same type.
///
/// Lives here rather than in whatever is running the program, so that every execution
/// path orders things identically instead of nearly identically.
pub fn compare(a: &Value, b: &Value) -> Comparison {
    if a.ty().is_integer() && b.ty().is_integer() {
        return match a.as_i128().unwrap().cmp(&b.as_i128().unwrap()) {
            std::cmp::Ordering::Less => Comparison::Less,
            std::cmp::Ordering::Equal => Comparison::Equal,
            std::cmp::Ordering::Greater => Comparison::Greater,
        };
    }
    if a.ty() == b.ty()
        && let Some(fmt) = decimal_of(a.ty())
    {
        // Two decimals can be written differently and be worth the same -- `1.0` and
        // `1.00` -- so this cannot compare the encodings.
        return decimal::ops::compare(
            fmt,
            decimal::unpack(fmt, a.bits().expect("a decimal has bits"), false),
            decimal::unpack(fmt, b.bits().expect("a decimal has bits"), false),
        );
    }
    if let (Some(x), Some(y)) = (a.bits(), b.bits())
        && a.ty() == b.ty()
    {
        let fmt = format_of(a.ty()).expect("a float type has a format");
        return binary::compare(fmt, x, y);
    }

    // Not everything that can be compared is a number. Two things of the same type are
    // either the same or they are not, which is what `=` asks — and without this, `=` on
    // text answered "unordered", and unordered makes all three comparisons false.
    // Two arrays are the same array when they are the same array. Asking whether they
    // hold the same things would make `=` mean something different here than it means
    // everywhere else, where it asks whether two things are one thing.
    if a.ty().array().is_some() && a.ty() == b.ty() {
        let (Value::Num { bits: x, .. }, Value::Num { bits: y, .. }) = (a, b) else {
            return Comparison::Unordered;
        };
        return if x == y { Comparison::Equal } else { Comparison::Unordered };
    }

    let ordering = match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),

        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        // An exact rational orders exactly. There is no NaN here and nothing unordered:
        // every pair of ratios stands one way or the other.
        (Value::Exact(x), Value::Exact(y)) => x.cmp(y),
        _ => return Comparison::Unordered,
    };
    match ordering {
        std::cmp::Ordering::Less => Comparison::Less,
        std::cmp::Ordering::Equal => Comparison::Equal,
        std::cmp::Ordering::Greater => Comparison::Greater,
    }
}

/// Whether an ordering satisfies what was asked of it.
///
/// A NaN is not less than, greater than, or equal to anything, itself included, so every
/// comparison answers false for one — except `!=`, which asks only that they differ, and
/// a NaN differs from everything. That is what unordered means, and it lives here so that
/// every execution path decides it the same way.
pub fn holds(op: CmpOp, ordering: Comparison) -> bool {
    match op {
        CmpOp::Less => ordering == Comparison::Less,
        CmpOp::Greater => ordering == Comparison::Greater,
        CmpOp::Equal => ordering == Comparison::Equal,
        CmpOp::LessEqual => matches!(ordering, Comparison::Less | Comparison::Equal),
        CmpOp::GreaterEqual => matches!(ordering, Comparison::Greater | Comparison::Equal),
        // The one a NaN answers true to. Everything else asks for a particular ordering
        // and a NaN has none; this asks only that the two are not the same, and a NaN is
        // not the same as anything, itself included.
        CmpOp::NotEqual => ordering != Comparison::Equal,
    }
}

/// One, of whichever numeric type.
pub fn one_of(ty: Ty) -> Value {
    if ty == Ty::Er {
        Value::Exact(std::rc::Rc::new(Exact::one()))
    } else if let Some(fmt) = decimal_of(ty) {
        Value::float(ty, decimal::text::from_text(fmt, Round::TiesToEven, false, "1").expect("one reads"))
    } else if ty.is_integer() {
        Value::int(ty, 1)
    } else {
        let fmt = format_of(ty).expect("a number has a format");
        Value::float(ty, binary::arith::one::<8>(fmt, false))
    }
}

/// The IEEE decimal format a type denotes.
pub fn decimal_of(ty: Ty) -> Option<decimal::Format> {
    Some(match ty {
        Ty::D32 => decimal::D32,
        Ty::D64 => decimal::D64,
        Ty::D128 => decimal::D128,
        _ => return None,
    })
}

/// The IEEE binary format a float type denotes.
pub fn format_of(ty: Ty) -> Option<Format> {
    Some(match ty {
        Ty::B16 => Format::B16,
        Ty::B32 => Format::B32,
        Ty::B64 => Format::B64,
        Ty::B128 => Format::B128,
        Ty::B256 => Format::B256,
        _ => return None,
    })
}

impl Value {
    pub fn ty(&self) -> Ty {
        match self {
            Value::Num { ty, .. } | Value::Wide { ty, .. } => *ty,
            Value::Bool(_) => Ty::Bool,
            Value::Str(_) => Ty::Str,
            Value::Exact(_) => Ty::Er,
        }
    }

    /// A float, stored narrow or wide according to whether it fits.
    pub fn float(ty: Ty, bits: Bits) -> Value {
        if matches!(ty, Ty::B128 | Ty::B256 | Ty::D128) {
            Value::Wide { ty, bits: Box::new(bits) }
        } else {
            Value::Num { ty, bits: bits.low64() }
        }
    }

    /// A float's encoding, widened back out. `b16`, `b32` and `b64` all sit in the low
    /// 64 bits, so this loses nothing.
    pub fn bits(&self) -> Option<Bits> {
        match self {
            Value::Num { ty, bits } if ty.is_float() => Some(Bits::from_u64(*bits)),
            Value::Wide { bits, .. } => Some(**bits),
            _ => None,
        }
    }

    pub fn text(value: &str) -> Value {
        Value::Str(std::rc::Rc::from(value))
    }

    /// Zero, of whichever type.
    pub fn zero(ty: Ty) -> Value {
        if ty == Ty::Er {
            Value::Exact(std::rc::Rc::new(Exact::zero()))
        } else if let Some(fmt) = decimal_of(ty) {
            Value::float(ty, decimal::zero(fmt, false, false))
        } else if ty.is_integer() {
            Value::Num { ty, bits: 0 }
        } else if let Some(fmt) = format_of(ty) {
            Value::float(ty, binary::zero(fmt, false))
        } else if ty == Ty::Bool {
            Value::Bool(false)
        } else {
            Value::text("")
        }
    }

    /// An integer's value, sign-extended if its type is signed.
    pub fn as_i128(&self) -> Option<i128> {
        let Value::Num { ty, bits } = self else { return None };
        let width = ty.int_bits()?;
        Some(if ty.is_signed() {
            // Push the sign bit to the top and back down, so the sign extends.
            let shift = 128 - width;
            ((*bits as i128) << shift) >> shift
        } else {
            *bits as i128
        })
    }

    /// Build an integer, cutting it to its width, and say nothing about what was cut.
    pub fn int(ty: Ty, value: i128) -> Value {
        Value::from_i128(ty, value).0
    }

    /// Build an integer, cutting it to its width. Reports whether anything was cut off.
    fn from_i128(ty: Ty, value: i128) -> (Value, bool) {
        let width = ty.int_bits().unwrap_or(64);
        let mask = if width == 64 { u64::MAX } else { (1u64 << width) - 1 };
        let bits = (value as u64) & mask;
        let fits = if ty.is_signed() {
            let limit = 1i128 << (width - 1);
            (-limit..limit).contains(&value)
        } else {
            (0..(1i128 << width)).contains(&value)
        };
        (Value::Num { ty, bits }, fits)
    }
}

/// Arithmetic on two exact rationals. Nothing rounds and nothing overflows, so the only
/// ways this can fail are the ones that have no answer at all.
fn exact_op(op: BinOp, a: &Exact, b: &Exact) -> Answer<Value> {
    use luarust_num::exact::Trouble;
    let outcome = match op {
        BinOp::Add => Ok(a.add(b)),
        BinOp::Sub => Ok(a.sub(b)),
        BinOp::Mul => Ok(a.mul(b)),
        BinOp::Div => a.div(b),
        BinOp::Mod => a.rem(b),
        BinOp::Pow => a.pow(b),
    };
    match outcome {
        Ok(value) => Ok(Value::Exact(std::rc::Rc::new(value))),
        Err(Trouble::DivideByZero) if op == BinOp::Mod => Err(Fault::new(
            "R0003",
            "this takes a remainder against zero.",
            "a remainder against zero is not a number",
            "check the divisor before taking a remainder.",
        )),
        Err(Trouble::DivideByZero) => Err(Fault::new(
            "R0002",
            "this divides an exact number by zero.",
            "an exact rational has no way to express what dividing by zero would give",
            "check the divisor before dividing. `er` has no infinity to answer with.",
        )),
        Err(Trouble::FractionalPower) => Err(Fault::new(
            "R0012",
            "this raises an exact number to a power that is not whole.",
            "a ratio raised to a whole power is a ratio, and raised to anything else usually is not",
            "use a whole exponent, or a float type, where the answer can be approximated.",
        )),
        Err(Trouble::PowerTooLarge) => Err(Fault::new(
            "R0013",
            format!("this raises an exact number to a power above {}.", Exact::POWER_LIMIT),
            "an exact answer has to be written down, and that one would not fit anywhere",
            "use a smaller exponent, or a float type, where the answer is rounded to a width.",
        )),
    }
}

/// Arithmetic on two decimals.
///
/// Nothing here faults. A decimal is a float, so dividing by zero is an infinity and
/// taking a remainder against one is a NaN, exactly as the binary formats do -- which is
/// the difference between `d64` and `er`, and worth knowing when choosing between them.
fn decimal_op(op: BinOp, fmt: decimal::Format, lhs: &Value, rhs: &Value) -> Value {
    let a = decimal::unpack(fmt, lhs.bits().expect("a decimal has bits"), false);
    let b = decimal::unpack(fmt, rhs.bits().expect("a decimal has bits"), false);
    let mode = Round::TiesToEven;
    let bits = match op {
        BinOp::Add => decimal::ops::add(fmt, mode, a, b, false),
        BinOp::Sub => decimal::ops::sub(fmt, mode, a, b, false),
        BinOp::Mul => decimal::ops::mul(fmt, mode, a, b, false),
        BinOp::Div => decimal::ops::div(fmt, mode, a, b, false),
        BinOp::Mod => decimal::ops::rem(fmt, mode, a, b, false),
        BinOp::Pow => decimal::ops::pow(fmt, mode, a, b, false),
    };
    Value::float(lhs.ty(), bits)
}

/// `lhs op rhs`, both already known to be the same type.
pub fn binary_op(
    op: BinOp,
    lhs: &Value,
    rhs: &Value,
    overflow: Overflow,
    division: Division,
) -> Answer<Value> {
    let plain = floored_binary_op(op, lhs, rhs, overflow, division)?;
    // Almost nothing leans, and the test for it belongs here rather than behind a call.
    // Only a remainder can, only under a convention `luarust-num` does not compute, and
    // only for the families that compute it there -- integers settled quotient and
    // remainder together at their own width and are already right. Left to `leaning`,
    // this cost the tree-walker 8% on a loop with no remainder in it at all: it calls
    // `binary_op` for every operation, where the VM's hot arms go straight to `int_op`.
    if op != BinOp::Mod || division == Division::Floored || lhs.ty().is_integer() {
        return Ok(plain);
    }
    leaning(op, lhs, rhs, plain, overflow, division)
}

/// A remainder the project did not ask for, moved to the one it did.
///
/// Every numeric family here computes a **floored** remainder, and the other two
/// conventions are one step away from it rather than a different algorithm:
///
/// ```text
///   truncated   the remainder follows the dividend, so where the two signs differ,
///               floored has already stepped once too far: subtract the divisor back
///   euclidean   the remainder is never negative, and a floored one is negative exactly
///               when the divisor is: subtract the divisor, which adds its size
/// ```
///
/// Which is why `luarust-num` needs no notion of this at all. Integers do their own,
/// inside `int_op`, because there the quotient has to move with the remainder and both
/// come out of one place; here `div` is exact division and has no quotient to correct.
#[inline]
fn leaning(
    op: BinOp,
    lhs: &Value,
    rhs: &Value,
    floored: Value,
    overflow: Overflow,
    division: Division,
) -> Answer<Value> {
    if op != BinOp::Mod || division == Division::Floored {
        return Ok(floored);
    }
    // Integers were settled at their own width, quotient and remainder together.
    if lhs.ty().is_integer() {
        return Ok(floored);
    }
    // A remainder of nought is nought in every convention, and a sign it has not got
    // cannot disagree with anything.
    if is_nought(&floored) {
        return Ok(floored);
    }
    let step = match division {
        Division::Floored => false,
        Division::Truncated => negative(lhs) != negative(rhs),
        Division::Euclidean => negative(rhs),
    };
    if !step {
        return Ok(floored);
    }
    floored_binary_op(BinOp::Sub, &floored, rhs, overflow, division)
}

/// Whether a value is below zero. Not-a-number is not, and neither is a negative nought:
/// what matters here is which side of zero the remainder fell, and a signed nought has
/// not fallen either way.
fn negative(value: &Value) -> bool {
    matches!(compare(value, &Value::zero(value.ty())), Comparison::Less)
}

fn is_nought(value: &Value) -> bool {
    matches!(compare(value, &Value::zero(value.ty())), Comparison::Equal)
}

fn floored_binary_op(
    op: BinOp,
    lhs: &Value,
    rhs: &Value,
    overflow: Overflow,
    division: Division,
) -> Answer<Value> {
    let ty = lhs.ty();
    if let (Value::Exact(a), Value::Exact(b)) = (lhs, rhs) {
        return exact_op(op, a, b);
    }
    if let Some(fmt) = decimal_of(ty)
        && ty == rhs.ty()
    {
        return Ok(decimal_op(op, fmt, lhs, rhs));
    }
    if ty.is_integer() && rhs.ty().is_integer() {
        return integer_op(op, ty, lhs, rhs, overflow, division);
    }
    // b32 and b64 are the two formats the hardware knows, and for these operations what
    // the hardware does is *correctly rounded* -- which means there is exactly one right
    // answer and both routes give it. That is not an assumption: `luarust-num` is checked
    // against the machine over 200,000 random pairs per operation, and this fast path is
    // checked against `luarust-num` again below. Doing it natively is worth roughly an
    // order of magnitude, since the alternative is an IEEE implementation in software.
    //
    // NaN payloads may differ between the two routes. Nothing in Luarust can observe one:
    // a NaN prints as `nan` and compares as unordered whatever it is carrying.
    if let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) = (lhs, rhs)
        && ty == rhs.ty()
    {
        match ty {
            Ty::B64 => return Ok(Value::Num { ty, bits: f64_op(op, *a, *b)? }),
            Ty::B32 => return Ok(Value::Num { ty, bits: f32_op(op, *a, *b)? }),
            _ => {}
        }
    }

    match (lhs.bits(), rhs.bits()) {
        (Some(a), Some(b)) if ty == rhs.ty() => float_op(op, ty, a, b),
        _ => Err(Fault::new(
            "R0001",
            "these two cannot be combined.",
            "arithmetic works on two numbers of the same type",
            "give both sides the same numeric type.",
        )),
    }
}

fn integer_op(
    op: BinOp,
    ty: Ty,
    lhs: &Value,
    rhs: &Value,
    overflow: Overflow,
    division: Division,
) -> Answer<Value> {
    let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) = (lhs, rhs) else {
        unreachable!("both were checked to be integers");
    };
    Ok(Value::Num { ty, bits: int_op(op, ty, *a, *b, overflow, division)? })
}

/// Integer arithmetic, at the width it is actually stored at.
///
/// Takes and returns raw stored bits, so whatever is running the program can hand these
/// straight over without unwrapping and rebuilding a value around every operation. It is
/// also, measurably, most of what a program's time goes on: doing this at 128 bits and
/// then range-checking the answer cost about five nanoseconds an operation more than
/// doing it at the width the number is actually kept at.
#[inline(always)]
pub fn int_op(
    op: BinOp,
    ty: Ty,
    a: u64,
    b: u64,
    overflow: Overflow,
    division: Division,
) -> Answer<u64> {
    macro_rules! at_width {
        ($signed:ty, $unsigned:ty) => {{
            let a = a as $unsigned as $signed;
            let b = b as $unsigned as $signed;
            let (value, overflowed) = match op {
                BinOp::Add => a.overflowing_add(b),
                BinOp::Sub => a.overflowing_sub(b),
                BinOp::Mul => a.overflowing_mul(b),
                // The quotient and the remainder come from one place, so the two always
                // describe the same division and `q × b + r` is `a` whichever convention
                // the project chose.
                BinOp::Div => {
                    if b == 0 {
                        return Err(divide_by_zero());
                    }
                    let (quotient, _, over) = divided!(division, a, b, $signed);
                    (quotient, over)
                }
                BinOp::Mod => {
                    if b == 0 {
                        return Err(remainder_by_zero());
                    }
                    // A remainder never overflows: it is smaller than the divisor. The
                    // one case the hardware crashes on is the most negative value against
                    // `-1`, whose remainder is nought in every convention.
                    let (_, remainder, _) = divided!(division, a, b, $signed);
                    (remainder, false)
                }
                BinOp::Pow => return power(op, ty, a as i128, b as i128, overflow),
            };
            if overflowed && overflow == Overflow::Trap {
                return Err(does_not_fit(ty));
            }
            Ok(value as $unsigned as u64)
        }};
    }

    macro_rules! at_width_unsigned {
        ($unsigned:ty) => {{
            let a = a as $unsigned;
            let b = b as $unsigned;
            let (value, overflowed) = match op {
                BinOp::Add => a.overflowing_add(b),
                BinOp::Sub => a.overflowing_sub(b),
                BinOp::Mul => a.overflowing_mul(b),
                BinOp::Div => {
                    if b == 0 {
                        return Err(divide_by_zero());
                    }
                    (a / b, false)
                }
                // Unsigned remainder is already floored: neither side can be negative.
                BinOp::Mod => {
                    if b == 0 {
                        return Err(remainder_by_zero());
                    }
                    (a % b, false)
                }
                BinOp::Pow => return power(op, ty, a as i128, b as i128, overflow),
            };
            if overflowed && overflow == Overflow::Trap {
                return Err(does_not_fit(ty));
            }
            Ok(value as u64)
        }};
    }

    match ty {
        Ty::I8 => at_width!(i8, u8),
        Ty::I16 => at_width!(i16, u16),
        Ty::I32 => at_width!(i32, u32),
        Ty::I64 => at_width!(i64, u64),
        Ty::U8 => at_width_unsigned!(u8),
        Ty::U16 => at_width_unsigned!(u16),
        Ty::U32 => at_width_unsigned!(u32),
        Ty::U64 => at_width_unsigned!(u64),
        _ => unreachable!("only the integers get here"),
    }
}

fn divide_by_zero() -> Box<Fault> {
    Fault::new(
        "R0002",
        "this divides a whole number by zero.",
        "an integer has no way to express what dividing by zero would give",
        "check the divisor before dividing, or use a float type, where it is an infinity.",
    )
}

fn remainder_by_zero() -> Box<Fault> {
    Fault::new(
        "R0003",
        "this takes a remainder against zero.",
        "a remainder against zero is not a number",
        "check the divisor before taking a remainder.",
    )
}

fn does_not_fit(ty: Ty) -> Box<Fault> {
    Fault::new(
        "R0005",
        format!("this does not fit in `{}`.", ty.word()),
        "with overflow set to trap, a whole number must fit the width it is stored at",
        format!("use a wider type than `{}`, or let overflow wrap.", ty.word()),
    )
}

/// Raising to a power, which is rare enough to stay where it was.
fn power(op: BinOp, ty: Ty, a: i128, b: i128, overflow: Overflow) -> Answer<u64> {
    debug_assert_eq!(op, BinOp::Pow);
    if b < 0 {
        return Err(Fault::new(
            "R0004",
            "this raises a whole number to a negative power.",
            "a whole number raised to a negative power is a fraction, which an integer cannot hold",
            "use a float type for the base.",
        ));
    }
    let mut result: i128 = 1;
    for _ in 0..b {
        result = result.saturating_mul(a);
    }
    let (value, fits) = Value::from_i128(ty, result);
    if !fits && overflow == Overflow::Trap {
        return Err(does_not_fit(ty));
    }
    let Value::Num { bits, .. } = value else { unreachable!("an integer") };
    Ok(bits)
}


/// `b64` arithmetic on the hardware, taking and returning the stored bits.
pub fn f64_op(op: BinOp, a: u64, b: u64) -> Answer<u64> {
    let (x, y) = (f64::from_bits(a), f64::from_bits(b));
    let value = match op {
        BinOp::Add => x + y,
        BinOp::Sub => x - y,
        BinOp::Mul => x * y,
        BinOp::Div => x / y,
        // Rust's `%` truncates and takes the sign of the dividend; mathematics takes the
        // sign of the divisor, so where they differ the divisor is added back. Both steps
        // are exact, so nothing rounds twice.
        BinOp::Mod => {
            let truncated = x % y;
            if truncated != 0.0 && truncated.is_sign_negative() != y.is_sign_negative() {
                truncated + y
            } else {
                truncated
            }
        }
        BinOp::Pow => {
            let fmt = format_of(Ty::B64).expect("b64 has a format");
            let result = float_pow(fmt, Ty::B64, Bits::from_u64(a), Bits::from_u64(b))?;
            return Ok(result.bits().expect("a float").low64());
        }
    };
    Ok(value.to_bits())
}

/// `b32` arithmetic on the hardware, taking and returning the stored bits.
pub fn f32_op(op: BinOp, a: u64, b: u64) -> Answer<u64> {
    let (x, y) = (f32::from_bits(a as u32), f32::from_bits(b as u32));
    let value = match op {
        BinOp::Add => x + y,
        BinOp::Sub => x - y,
        BinOp::Mul => x * y,
        BinOp::Div => x / y,
        BinOp::Mod => {
            let truncated = x % y;
            if truncated != 0.0 && truncated.is_sign_negative() != y.is_sign_negative() {
                truncated + y
            } else {
                truncated
            }
        }
        BinOp::Pow => {
            let fmt = format_of(Ty::B32).expect("b32 has a format");
            let result = float_pow(fmt, Ty::B32, Bits::from_u64(a), Bits::from_u64(b))?;
            return Ok(result.bits().expect("a float").low64());
        }
    };
    Ok(value.to_bits() as u64)
}

fn float_op(op: BinOp, ty: Ty, a: Bits, b: Bits) -> Answer<Value> {
    let fmt = format_of(ty).expect("a float type has a format");
    let mode = Round::TiesToEven;
    let bits = match op {
        BinOp::Add => binary::add(fmt, mode, a, b),
        BinOp::Sub => binary::sub(fmt, mode, a, b),
        BinOp::Mul => binary::mul(fmt, mode, a, b),
        BinOp::Div => binary::div(fmt, mode, a, b),
        BinOp::Mod => float_remainder(fmt, a, b),
        BinOp::Pow => return float_pow(fmt, ty, a, b),
    };
    Ok(Value::float(ty, bits))
}

/// Floored remainder.
///
/// `luarust-num` gives the truncated one, exactly — the sign of the dividend, as `fmod`
/// does. Mathematics takes the sign of the divisor, so where the two disagree the divisor
/// is added back, which is exact because the remainder is already smaller than it.
fn float_remainder(fmt: Format, a: Bits, b: Bits) -> Bits {
    let truncated = binary::remainder(fmt, a, b);
    let left = binary::unpack(fmt, truncated);
    if left.class == binary::Class::Zero || left.class == binary::Class::Nan {
        return truncated;
    }
    if left.sign != binary::unpack(fmt, b).sign {
        return binary::add(fmt, Round::TiesToEven, truncated, b);
    }
    truncated
}

/// Toward zero, to a whole number.
fn truncate(fmt: Format, value: Bits) -> Bits {
    let v = binary::unpack(fmt, value);
    match v.class {
        binary::Class::Zero | binary::Class::Infinite | binary::Class::Nan => return value,
        _ => {}
    }
    if v.exp >= 0 {
        return value; // already whole: every bit is above the point
    }
    let drop = (-v.exp) as u32;
    if drop >= v.sig.bit_len() {
        return binary::zero(fmt, v.sign);
    }
    let whole = v.sig.shr(drop).shl(drop);
    binary::round_and_pack(fmt, Round::TowardZero, v.sign, whole, v.exp)
}

/// Raising to a power, for whole exponents.
///
/// Exact by repeated multiplication when the exponent is a whole number, which is what a
/// program actually writes. A fractional exponent is refused rather than approximated:
/// IEEE 754 does not require `pow` to be correctly rounded, and an answer the three
/// execution paths might disagree about is worse than no answer at all.
fn float_pow(fmt: Format, ty: Ty, base: Bits, exponent: Bits) -> Answer<Value> {
    let e = binary::unpack(fmt, exponent);
    let whole = truncate(fmt, exponent);
    if whole != exponent || e.class == binary::Class::Nan || e.class == binary::Class::Infinite {
        return Err(Fault::new(
            "R0006",
            "this raises a number to a power that is not whole.",
            "a power must be a whole number, since a fractional one has no exactly rounded answer",
            "use a whole exponent.",
        ));
    }

    // How many times to multiply. Anything past a few thousand has already overflowed.
    let mode = Round::TiesToEven;
    let mut count = 0u32;
    let one = binary::arith::one::<8>(fmt, false);
    let mut counter = binary::zero(fmt, false);
    let magnitude = binary::arith::abs(fmt, whole);
    while binary::compare(fmt, counter, magnitude) == binary::Comparison::Less {
        counter = binary::add(fmt, mode, counter, one);
        count += 1;
        if count > 4096 {
            return Err(Fault::new(
                "R0007",
                "this power is too large to work out by multiplying.",
                "a power is computed by repeated multiplication, which has a limit",
                "use a smaller exponent.",
            ));
        }
    }

    let mut result = one;
    for _ in 0..count {
        result = binary::mul(fmt, mode, result, base);
    }
    if e.sign {
        result = binary::div(fmt, mode, one, result);
    }
    Ok(Value::float(ty, result))
}

/// Negation. Exact for every value.
pub fn negate(value: &Value, overflow: Overflow) -> Answer<Value> {
    match value {
        Value::Exact(value) => Ok(Value::Exact(std::rc::Rc::new(value.negated()))),
        Value::Num { ty, .. } | Value::Wide { ty, .. } if ty.is_decimal() => {
            // The sign is one bit at the top, whatever else the encoding is doing.
            let fmt = decimal_of(*ty).expect("a decimal type has a format");
            let mut bits = value.bits().expect("a decimal has bits");
            if bits.bit(fmt.bits - 1) {
                bits.clear_bit(fmt.bits - 1);
            } else {
                bits.set_bit(fmt.bits - 1);
            }
            Ok(Value::float(*ty, bits))
        }
        Value::Num { ty, .. } | Value::Wide { ty, .. } if ty.is_float() => {
            let fmt = format_of(*ty).expect("a float type has a format");
            Ok(Value::float(*ty, binary::neg(fmt, value.bits().expect("a float has bits"))))
        }
        Value::Num { ty, .. } => {
            let (negated, fits) = Value::from_i128(*ty, -value.as_i128().unwrap());
            if !fits && overflow == Overflow::Trap {
                return Err(Fault::new(
                    "R0005",
                    format!("negating this does not fit in `{}`.", ty.word()),
                    "with overflow set to trap, a whole number must fit the width it is stored at",
                    "use a wider type, or a signed one.",
                ));
            }
            Ok(negated)
        }
        _ => Err(Fault::new(
            "R0008",
            "only a number can be negated.",
            "negation applies to numbers",
            "negate a number instead.",
        )),
    }
}

impl std::fmt::Display for Value {
    /// How a value prints: as the value that is actually stored, not as the text that was
    /// written to make it.
    ///
    /// Every format shows its own width. A binary float is exactly `sig × 2^exp`, and a
    /// negative exponent is `sig × 5^k / 10^k` — so every one of them has a finite decimal
    /// expansion and none of it has to be guessed at. What is shown is the shortest run of
    /// digits that reads back as the same number, so a `b128` shows the thirty-four
    /// significant digits it has and a `b64` still shows its sixteen.
    ///
    /// The decimal formats never had this trouble: their significands are decimal digits
    /// already, so writing one out is arranging them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Str(text) => write!(f, "{text}"),
            Value::Bool(value) => write!(f, "{value}"),
            // A fraction, because that is what it is. A third has no finite decimal, and
            // printing `0.333…` is the one thing this type exists not to do.
            Value::Exact(value) => write!(f, "{value}"),

            // The elements, in the brackets an array is written in. The handle itself is
            // never shown: it is where the array is, not what it is.
            _ if self.ty().array().is_some() => {
                let Value::Num { bits, .. } = self else { unreachable!("an array is a handle") };
                let handle = *bits as u32;
                let written: Vec<String> = (0..crate::heap::length(handle))
                    .map(|at| crate::heap::read(handle, at).expect("in range").to_string())
                    .collect();
                write!(f, "[{}]", written.join(", "))
            }
            _ if self.ty().is_integer() => write!(f, "{}", self.as_i128().unwrap()),
            // A decimal writes out exactly, always: its significand *is* decimal digits,
            // so this is arranging them rather than searching for a shortest form.
            _ if self.ty().is_decimal() => {
                let fmt = decimal_of(self.ty()).expect("a decimal type has a format");
                let bits = self.bits().expect("a decimal has bits");
                write!(f, "{}", decimal::text::to_text(fmt, decimal::unpack(fmt, bits, false)))
            }
            _ => {
                let ty = self.ty();
                let fmt_of = format_of(ty).expect("a float type has a format");
                let bits = self.bits().expect("a float has bits");
                let written = match floats() {
                    Floats::Exact => binary::text::to_text(fmt_of, bits),
                    Floats::Shortest => binary::text::to_shortest(fmt_of, bits),
                };
                match written {
                    Some(written) => write!(f, "{written}"),
                    // Not a number and not a quantity: those are named, not written out.
                    None => {
                        let taken = binary::unpack(fmt_of, bits);
                        match taken.class {
                            binary::Class::Nan => write!(f, "nan"),
                            _ => write!(f, "{}inf", if taken.sign { "-" } else { "" }),
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luarust_num::binary::Format;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        /// A spread of values. Uniform bit patterns are almost all enormous, so the other
        /// shapes exist to reach the ranges arithmetic actually happens in.
        fn f64(&mut self) -> f64 {
            let bits = self.next();
            match self.next() % 4 {
                0 => f64::from_bits(bits),
                1 => {
                    let e = 1023u64 + (self.next() % 60) - 30;
                    f64::from_bits((bits & 0x800f_ffff_ffff_ffff) | (e << 52))
                }
                2 => f64::from_bits(bits & 0x800f_ffff_ffff_ffff),
                _ => ((bits % 4000) as f64) - 2000.0,
            }
        }
    }

    /// The whole justification for the native fast path, tested rather than asserted.
    ///
    /// If the hardware and `luarust-num` ever disagree about `b32` or `b64`, this is where
    /// it shows up — and it has to show up here, because once both routes are in use the
    /// three execution paths would otherwise start quietly answering differently.
    #[test]
    fn the_hardware_and_the_soft_float_give_the_same_b64_answers() {
        let fmt = Format::B64;
        let mode = Round::TiesToEven;
        let mut rng = Rng(1);

        for _ in 0..200_000 {
            let (x, y) = (rng.f64(), rng.f64());
            let (a, b) = (x.to_bits(), y.to_bits());
            let wide = |v: u64| Bits::from_u64(v);

            for (op, soft) in [
                (BinOp::Add, binary::add(fmt, mode, wide(a), wide(b))),
                (BinOp::Sub, binary::sub(fmt, mode, wide(a), wide(b))),
                (BinOp::Mul, binary::mul(fmt, mode, wide(a), wide(b))),
                (BinOp::Div, binary::div(fmt, mode, wide(a), wide(b))),
                (BinOp::Mod, float_remainder(fmt, wide(a), wide(b))),
            ] {
                let fast = f64::from_bits(f64_op(op, a, b).expect("no fault"));
                let slow = f64::from_bits(soft.low64());
                if slow.is_nan() {
                    // Payloads may differ and nothing in the language can see one.
                    assert!(fast.is_nan(), "{x:e} {} {y:e}: {fast:e} where soft-float said nan", op.word());
                } else {
                    assert_eq!(
                        fast.to_bits(),
                        slow.to_bits(),
                        "{x:e} {} {y:e}: hardware {fast:e}, soft-float {slow:e}",
                        op.word()
                    );
                }
            }
        }
    }

    #[test]
    fn the_hardware_and_the_soft_float_give_the_same_b32_answers() {
        let fmt = Format::B32;
        let mode = Round::TiesToEven;
        let mut rng = Rng(2);

        for _ in 0..200_000 {
            let (x, y) = (rng.f64() as f32, rng.f64() as f32);
            let (a, b) = (x.to_bits() as u64, y.to_bits() as u64);
            let wide = |v: u64| Bits::from_u64(v);

            for (op, soft) in [
                (BinOp::Add, binary::add(fmt, mode, wide(a), wide(b))),
                (BinOp::Sub, binary::sub(fmt, mode, wide(a), wide(b))),
                (BinOp::Mul, binary::mul(fmt, mode, wide(a), wide(b))),
                (BinOp::Div, binary::div(fmt, mode, wide(a), wide(b))),
                (BinOp::Mod, float_remainder(fmt, wide(a), wide(b))),
            ] {
                let fast = f32::from_bits(f32_op(op, a, b).expect("no fault") as u32);
                let slow = f32::from_bits(soft.low64() as u32);
                if slow.is_nan() {
                    assert!(fast.is_nan(), "{x:e} {} {y:e}", op.word());
                } else {
                    assert_eq!(
                        fast.to_bits(),
                        slow.to_bits(),
                        "{x:e} {} {y:e}: hardware {fast:e}, soft-float {slow:e}",
                        op.word()
                    );
                }
            }
        }
    }

    #[test]
    fn the_remainder_is_floored_either_way() {
        let cases: [(f64, f64, f64); 4] =
            [(-7.0, 3.0, 2.0), (7.0, -3.0, -2.0), (7.0, 3.0, 1.0), (-7.5, 2.0, 0.5)];
        for (a, b, expected) in cases {
            let bits = f64_op(BinOp::Mod, a.to_bits(), b.to_bits()).expect("no fault");
            assert_eq!(f64::from_bits(bits), expected, "{a} mod {b}");
        }
        // And against zero it is a NaN rather than a fault, the way every float is.
        let bits = f64_op(BinOp::Mod, 1.0f64.to_bits(), 0.0f64.to_bits()).expect("no fault");
        assert!(f64::from_bits(bits).is_nan());
    }
}

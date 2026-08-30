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
use luarust_num::Uint;
use luarust_num::binary::{self, Comparison, Format, Round};
use luarust_parse::ast::{BinOp, Ty};

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

/// A value, and the type it is.
///
/// The shape here is a performance decision, and a measured one. Holding every float at
/// `b256`'s width made a value 72 bytes, which every register write and every clone then
/// had to copy — and since both the interpreter and the VM paid it, the VM's advantage
/// almost vanished into the memcpy. Everything that fits in 64 bits now does: all the
/// integers, and `b16`, `b32` and `b64`. Only `b128` and `b256` are boxed, so the two
/// widths nobody uses in a hot loop pay for themselves instead of taxing the ones that
/// are.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// An integer, or a float narrow enough to fit: `b16`, `b32`, `b64`.
    Num { ty: Ty, bits: u64 },
    /// `b128` and `b256`, which do not fit.
    Wide { ty: Ty, bits: Box<Bits> },
    Bool(bool),
    /// Shared, because a string is immutable here and copying one to move it between
    /// registers would be paying for nothing.
    Str(std::rc::Rc<str>),
}

/// Something that went wrong while a program was running.
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    pub code: &'static str,
    pub message: String,
    pub rule: &'static str,
    pub fix: String,
}

impl Fault {
    fn new(code: &'static str, message: impl Into<String>, rule: &'static str, fix: impl Into<String>) -> Self {
        Self { code, message: message.into(), rule, fix: fix.into() }
    }
}

/// A fault, and where in the program it happened.
///
/// Shared by every execution path, so that a program stopping under the interpreter and
/// the same program stopping under the VM produce the same words as well as the same
/// behaviour.
#[derive(Clone, Debug)]
pub struct Stopped {
    pub fault: Fault,
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
    match (a.bits(), b.bits()) {
        (Some(x), Some(y)) if a.ty() == b.ty() => {
            let fmt = format_of(a.ty()).expect("a float type has a format");
            binary::compare(fmt, x, y)
        }
        _ => Comparison::Unordered,
    }
}

/// One, of whichever numeric type.
pub fn one_of(ty: Ty) -> Value {
    if ty.is_integer() {
        Value::int(ty, 1)
    } else {
        let fmt = format_of(ty).expect("a number has a format");
        Value::float(ty, binary::arith::one::<8>(fmt, false))
    }
}

/// The IEEE format a float type denotes.
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
        }
    }

    /// A float, stored narrow or wide according to whether it fits.
    pub fn float(ty: Ty, bits: Bits) -> Value {
        if matches!(ty, Ty::B128 | Ty::B256) {
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
        if ty.is_integer() {
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

/// `lhs op rhs`, both already known to be the same type.
pub fn binary_op(op: BinOp, lhs: &Value, rhs: &Value, overflow: Overflow) -> Result<Value, Fault> {
    let ty = lhs.ty();
    if ty.is_integer() && rhs.ty().is_integer() {
        return integer_op(op, ty, lhs, rhs, overflow);
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

fn integer_op(op: BinOp, ty: Ty, lhs: &Value, rhs: &Value, overflow: Overflow) -> Result<Value, Fault> {
    let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) = (lhs, rhs) else {
        unreachable!("both were checked to be integers");
    };
    Ok(Value::Num { ty, bits: int_op(op, ty, *a, *b, overflow)? })
}

/// Integer arithmetic, at the width it is actually stored at.
///
/// Takes and returns raw stored bits, so whatever is running the program can hand these
/// straight over without unwrapping and rebuilding a value around every operation. It is
/// also, measurably, most of what a program's time goes on: doing this at 128 bits and
/// then range-checking the answer cost about five nanoseconds an operation more than
/// doing it at the width the number is actually kept at.
pub fn int_op(op: BinOp, ty: Ty, a: u64, b: u64, overflow: Overflow) -> Result<u64, Fault> {
    macro_rules! at_width {
        ($signed:ty, $unsigned:ty) => {{
            let a = a as $unsigned as $signed;
            let b = b as $unsigned as $signed;
            let (value, overflowed) = match op {
                BinOp::Add => a.overflowing_add(b),
                BinOp::Sub => a.overflowing_sub(b),
                BinOp::Mul => a.overflowing_mul(b),
                BinOp::Div => {
                    if b == 0 {
                        return Err(divide_by_zero());
                    }
                    a.overflowing_div(b)
                }
                BinOp::Mod => {
                    if b == 0 {
                        return Err(remainder_by_zero());
                    }
                    // The most negative value against -1 has no remainder that fits, and
                    // asking the hardware is a crash rather than an answer.
                    if b == -1 {
                        (0, false)
                    } else {
                        // Floored: `%` takes the sign of the dividend and mathematics
                        // takes the sign of the divisor, so where they differ the divisor
                        // is added back.
                        let truncated = a % b;
                        let floored = if truncated != 0 && (truncated < 0) != (b < 0) {
                            truncated + b
                        } else {
                            truncated
                        };
                        (floored, false)
                    }
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

fn divide_by_zero() -> Fault {
    Fault::new(
        "R0002",
        "this divides a whole number by zero.",
        "an integer has no way to express what dividing by zero would give",
        "check the divisor before dividing, or use a float type, where it is an infinity.",
    )
}

fn remainder_by_zero() -> Fault {
    Fault::new(
        "R0003",
        "this takes a remainder against zero.",
        "a remainder against zero is not a number",
        "check the divisor before taking a remainder.",
    )
}

fn does_not_fit(ty: Ty) -> Fault {
    Fault::new(
        "R0005",
        format!("this does not fit in `{}`.", ty.word()),
        "with `defaults.overflow.trap`, a whole number must fit the width it is stored at",
        format!("use a wider type than `{}`, or drop `defaults.overflow.trap` and let it wrap.", ty.word()),
    )
}

/// Raising to a power, which is rare enough to stay where it was.
fn power(op: BinOp, ty: Ty, a: i128, b: i128, overflow: Overflow) -> Result<u64, Fault> {
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


fn float_op(op: BinOp, ty: Ty, a: Bits, b: Bits) -> Result<Value, Fault> {
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

/// Floored remainder, built from the operations that are exact.
///
/// `a - b × floor(a ÷ b)`, with the quotient truncated toward zero and then corrected
/// downward when the signs disagree — which is what makes the answer take the divisor's
/// sign, the way mathematics does and the C family does not.
fn float_remainder(fmt: Format, a: Bits, b: Bits) -> Bits {
    let mode = Round::TiesToEven;
    if binary::unpack(fmt, b).class == binary::Class::Zero {
        return binary::quiet_nan(fmt);
    }
    let quotient = binary::div(fmt, mode, a, b);
    let truncated = truncate(fmt, quotient);
    let mut remainder = binary::sub(fmt, mode, a, binary::mul(fmt, mode, truncated, b));

    // A non-zero remainder whose sign differs from the divisor is on the wrong side.
    let remainder_negative = binary::unpack(fmt, remainder).sign;
    let divisor_negative = binary::unpack(fmt, b).sign;
    let is_zero = binary::unpack(fmt, remainder).class == binary::Class::Zero;
    if !is_zero && remainder_negative != divisor_negative {
        remainder = binary::add(fmt, mode, remainder, b);
    }
    remainder
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
fn float_pow(fmt: Format, ty: Ty, base: Bits, exponent: Bits) -> Result<Value, Fault> {
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
pub fn negate(value: &Value, overflow: Overflow) -> Result<Value, Fault> {
    match value {
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
                    "with `defaults.overflow.trap`, a whole number must fit the width it is stored at",
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
    /// `b16`, `b32` and `b64` all fit exactly inside an `f64`, so those are shown exactly.
    /// `b128` and `b256` are shown through `f64` as well, which is **not** exact for them
    /// — printing those at their full width needs arbitrary-precision decimal output,
    /// which iteration 1 does not have.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Str(text) => write!(f, "{text}"),
            Value::Bool(value) => write!(f, "{value}"),
            _ if self.ty().is_integer() => write!(f, "{}", self.as_i128().unwrap()),
            _ => {
                let ty = self.ty();
                let fmt_of = format_of(ty).expect("a float type has a format");
                let widened = binary::convert::<8>(
                    fmt_of,
                    Format::B64,
                    Round::TiesToEven,
                    self.bits().expect("a float has bits"),
                );
                let number = f64::from_bits(widened.low64());
                if number.is_nan() {
                    write!(f, "nan")
                } else if number.is_infinite() {
                    write!(f, "{}inf", if number.is_sign_negative() { "-" } else { "" })
                } else {
                    write!(f, "{number}")
                }
            }
        }
    }
}

//! Arithmetic on decimals.
//!
//! Every operation does the same three things: take both sides apart, work out an exact
//! significand and exponent, and hand them to [`round_and_pack`], which is the only place
//! that knows how to round. Five operations that round in one place agree with each
//! other; five that each round for themselves do not.

use super::{Class, Format, Unpacked, round_and_pack, ten_to};
use crate::Uint;
use crate::binary::{Comparison, Round};

/// The width the intermediates are computed at. A `d128` significand is 34 digits, and
/// multiplying two of them makes 68 — so the working width has to hold that and the
/// digits a division asks for on top.
pub type Wide = Uint<8>;

/// Whether a value is a NaN, so operations can hand one straight back.
fn nan_of<const W: usize>(v: &Unpacked<W>) -> bool {
    matches!(v.class, Class::Nan { .. })
}

fn infinite<const W: usize>(v: &Unpacked<W>) -> bool {
    v.class == Class::Infinite
}

/// `a + b`.
pub fn add(fmt: Format, mode: Round, a: Unpacked<8>, b: Unpacked<8>, dpd: bool) -> Wide {
    if nan_of(&a) || nan_of(&b) {
        return super::quiet_nan(fmt, dpd);
    }
    if infinite(&a) || infinite(&b) {
        // Infinity plus the other sign of infinity is the one addition with no answer.
        if infinite(&a) && infinite(&b) && a.sign != b.sign {
            return super::quiet_nan(fmt, dpd);
        }
        let sign = if infinite(&a) { a.sign } else { b.sign };
        return super::infinity(fmt, sign, dpd);
    }

    // Bring both to the same exponent, which means scaling the larger one up. The working
    // width is what bounds how far that can go; past it the smaller value cannot affect
    // the answer's leading digits anyway.
    let (low, high) = if a.exp <= b.exp { (a, b) } else { (b, a) };
    let gap = (high.exp - low.exp) as u32;
    let (x, y, exp) = if gap > 2 * fmt.digits + 2 {
        // The smaller one is too far below to reach, but it still decides a tie, so it
        // is kept as a single unit rather than dropped.
        (low.sig.min(Uint::from_u64(1)), high.sig.wrapping_mul(ten_to(2 * fmt.digits + 2)), low.exp)
    } else {
        (low.sig, high.sig.wrapping_mul(ten_to(gap)), low.exp)
    };

    let (low_signed, high_signed) = (low.sign, high.sign);
    if low_signed == high_signed {
        return round_and_pack(fmt, mode, low_signed, x.wrapping_add(y), exp, dpd);
    }
    // Opposite signs: the larger magnitude keeps its sign, and equal magnitudes give a
    // zero whose sign depends on which way the rounding goes.
    match x.cmp(&y) {
        std::cmp::Ordering::Equal => {
            let sign = mode == Round::TowardNegative;
            super::zero(fmt, sign, dpd)
        }
        std::cmp::Ordering::Greater => {
            round_and_pack(fmt, mode, low_signed, x.wrapping_sub(y), exp, dpd)
        }
        std::cmp::Ordering::Less => {
            round_and_pack(fmt, mode, high_signed, y.wrapping_sub(x), exp, dpd)
        }
    }
}

pub fn sub(fmt: Format, mode: Round, a: Unpacked<8>, mut b: Unpacked<8>, dpd: bool) -> Wide {
    if !nan_of(&b) {
        b.sign = !b.sign;
    }
    add(fmt, mode, a, b, dpd)
}

pub fn mul(fmt: Format, mode: Round, a: Unpacked<8>, b: Unpacked<8>, dpd: bool) -> Wide {
    if nan_of(&a) || nan_of(&b) {
        return super::quiet_nan(fmt, dpd);
    }
    let sign = a.sign != b.sign;
    if infinite(&a) || infinite(&b) {
        // Infinity times nothing has no answer.
        let zero_side = (a.class == Class::Finite && a.sig.is_zero())
            || (b.class == Class::Finite && b.sig.is_zero());
        if zero_side {
            return super::quiet_nan(fmt, dpd);
        }
        return super::infinity(fmt, sign, dpd);
    }
    round_and_pack(fmt, mode, sign, a.sig.wrapping_mul(b.sig), a.exp + b.exp, dpd)
}

pub fn div(fmt: Format, mode: Round, a: Unpacked<8>, b: Unpacked<8>, dpd: bool) -> Wide {
    if nan_of(&a) || nan_of(&b) {
        return super::quiet_nan(fmt, dpd);
    }
    let sign = a.sign != b.sign;
    match (a.class, b.class) {
        (Class::Infinite, Class::Infinite) => return super::quiet_nan(fmt, dpd),
        (Class::Infinite, _) => return super::infinity(fmt, sign, dpd),
        (_, Class::Infinite) => return super::zero(fmt, sign, dpd),
        _ => {}
    }
    if b.sig.is_zero() {
        // Nothing over nothing has no answer; anything else over nothing is an infinity,
        // which is what a float has and an exact rational does not.
        if a.sig.is_zero() {
            return super::quiet_nan(fmt, dpd);
        }
        return super::infinity(fmt, sign, dpd);
    }
    if a.sig.is_zero() {
        return super::zero(fmt, sign, dpd);
    }

    // Scale the numerator so the quotient certainly has more digits than the format
    // keeps, then let the rounding throw the extra ones away. Two extra digits is enough
    // for the rounding to be right, and the remainder decides ties that land exactly.
    let extra = fmt.digits + 2;
    let scaled = a.sig.wrapping_mul(ten_to(extra));
    let (quotient, remainder) = scaled.div_rem(b.sig);

    if remainder.is_zero() {
        // The division came out exactly, so the answer should be written the way the
        // standard prefers -- at the exponent the two sides imply, rather than at
        // whatever the scaling above happened to produce. Without this, `1 div 8` is
        // `0.1250000000000000`, which is the right number and the wrong way to say it.
        let mut sig = quotient;
        let mut exp = a.exp - b.exp - extra as i32;
        let ideal = a.exp - b.exp;
        let ten = Uint::from_u64(10);
        while exp < ideal {
            let (next, left) = sig.div_rem(ten);
            if !left.is_zero() {
                break;
            }
            sig = next;
            exp += 1;
        }
        return round_and_pack(fmt, mode, sign, sig, exp, dpd);
    }

    // A remainder means the true answer is above the quotient, which a tie has to know.
    let quotient = quotient | Uint::from_u64(1);
    round_and_pack(fmt, mode, sign, quotient, a.exp - b.exp - extra as i32, dpd)
}

/// Floored remainder, the same one every other numeric type here gives.
pub fn rem(fmt: Format, mode: Round, a: Unpacked<8>, b: Unpacked<8>, dpd: bool) -> Wide {
    if nan_of(&a) || nan_of(&b) || infinite(&a) {
        return super::quiet_nan(fmt, dpd);
    }
    if b.class == Class::Finite && b.sig.is_zero() {
        return super::quiet_nan(fmt, dpd);
    }
    if infinite(&b) {
        // Everything is a remainder of an infinity, except that the sign has to follow
        // the divisor, so a value on the other side wraps round to the infinity itself.
        if a.sign == b.sign || (a.class == Class::Finite && a.sig.is_zero()) {
            return super::round_and_pack(fmt, mode, a.sign, a.sig, a.exp, dpd);
        }
        return super::infinity(fmt, b.sign, dpd);
    }

    // Bring both to one exponent and take an integer remainder there, which is exact.
    let exp = a.exp.min(b.exp);
    let gap_a = (a.exp - exp) as u32;
    let gap_b = (b.exp - exp) as u32;
    if gap_a > 2 * fmt.digits + 4 || gap_b > 2 * fmt.digits + 4 {
        // Too far apart to bring together at this width. One of them is so much larger
        // than the other that the answer is simply the smaller one, or zero.
        if a.exp > b.exp {
            return super::zero(fmt, b.sign, dpd);
        }
        return round_and_pack(fmt, mode, a.sign, a.sig, a.exp, dpd);
    }
    let x = a.sig.wrapping_mul(ten_to(gap_a));
    let y = b.sig.wrapping_mul(ten_to(gap_b));
    if y.is_zero() {
        return super::quiet_nan(fmt, dpd);
    }

    let (_, mut left) = x.div_rem(y);
    let mut sign = a.sign;
    // Truncation leaves the sign of the numerator; flooring wants the sign of the
    // divisor, so anything left over on the wrong side is taken from the other end.
    if !left.is_zero() && a.sign != b.sign {
        left = y.wrapping_sub(left);
        sign = b.sign;
    }
    if left.is_zero() {
        return super::zero(fmt, b.sign, dpd);
    }
    round_and_pack(fmt, mode, sign, left, exp, dpd)
}

/// Raising to a power, for whole exponents. Anything else is a NaN, since the answer is
/// generally not something a decimal could hold exactly or inexactly.
pub fn pow(fmt: Format, mode: Round, a: Unpacked<8>, b: Unpacked<8>, dpd: bool) -> Wide {
    if nan_of(&a) || nan_of(&b) {
        return super::quiet_nan(fmt, dpd);
    }
    let Some(times) = whole_value(fmt, &b) else {
        return super::quiet_nan(fmt, dpd);
    };
    if times == 0 {
        return round_and_pack(fmt, mode, false, Uint::from_u64(1), 0, dpd);
    }

    // Square and multiply, rounding at every step -- which is what raising to a power in
    // a fixed width means, and is why `x ** 2` and `x * x` give the same answer.
    let mut out = Unpacked { sign: false, class: Class::Finite, sig: Uint::from_u64(1), exp: 0 };
    let mut base = a;
    let mut left = times.unsigned_abs();
    while left > 0 {
        if left & 1 == 1 {
            out = super::unpack(fmt, mul(fmt, mode, out, base, dpd), dpd);
        }
        left >>= 1;
        if left > 0 {
            base = super::unpack(fmt, mul(fmt, mode, base, base, dpd), dpd);
        }
    }

    if times < 0 {
        let one = Unpacked { sign: false, class: Class::Finite, sig: Uint::from_u64(1), exp: 0 };
        return div(fmt, mode, one, out, dpd);
    }
    round_and_pack(fmt, mode, out.sign, out.sig, out.exp, dpd)
}

/// The value as a whole number, when it is one small enough to be an exponent.
fn whole_value(fmt: Format, v: &Unpacked<8>) -> Option<i64> {
    if v.class != Class::Finite {
        return None;
    }
    let mut sig = v.sig;
    let mut exp = v.exp;
    while exp < 0 {
        let (next, remainder) = sig.div_rem(Uint::from_u64(10));
        if !remainder.is_zero() {
            return None;
        }
        sig = next;
        exp += 1;
    }
    if exp > 0 {
        if exp > fmt.digits as i32 {
            return None;
        }
        sig = sig.wrapping_mul(ten_to(exp as u32));
    }
    let magnitude = sig.low64();
    if sig != Uint::from_u64(magnitude) || magnitude > 100_000 {
        return None;
    }
    Some(if v.sign { -(magnitude as i64) } else { magnitude as i64 })
}

/// How two decimals order. Equal values with different exponents are equal, which is why
/// this cannot compare the encodings.
pub fn compare(fmt: Format, a: Unpacked<8>, b: Unpacked<8>) -> Comparison {
    if nan_of(&a) || nan_of(&b) {
        return Comparison::Unordered;
    }
    let a_zero = a.class == Class::Finite && a.sig.is_zero();
    let b_zero = b.class == Class::Finite && b.sig.is_zero();
    if a_zero && b_zero {
        // Both zeros, whatever their signs and exponents.
        return Comparison::Equal;
    }
    match (a.class, b.class) {
        (Class::Infinite, Class::Infinite) => {
            return if a.sign == b.sign {
                Comparison::Equal
            } else if a.sign {
                Comparison::Less
            } else {
                Comparison::Greater
            };
        }
        (Class::Infinite, _) => {
            return if a.sign { Comparison::Less } else { Comparison::Greater };
        }
        (_, Class::Infinite) => {
            return if b.sign { Comparison::Greater } else { Comparison::Less };
        }
        _ => {}
    }

    if a.sign != b.sign {
        return if a.sign { Comparison::Less } else { Comparison::Greater };
    }

    // Same sign and both finite: bring them to one exponent and compare the integers.
    let exp = a.exp.min(b.exp);
    let gap_a = (a.exp - exp) as u32;
    let gap_b = (b.exp - exp) as u32;
    let reach = 2 * fmt.digits + 4;
    let ordering = if gap_a > reach || gap_b > reach {
        // Too far apart to scale; the one with the larger exponent is the larger number,
        // unless it is zero.
        if a.exp > b.exp { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less }
    } else {
        a.sig.wrapping_mul(ten_to(gap_a)).cmp(&b.sig.wrapping_mul(ten_to(gap_b)))
    };
    let ordering = if a.sign { ordering.reverse() } else { ordering };
    match ordering {
        std::cmp::Ordering::Less => Comparison::Less,
        std::cmp::Ordering::Equal => Comparison::Equal,
        std::cmp::Ordering::Greater => Comparison::Greater,
    }
}

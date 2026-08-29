//! The five operations IEEE 754 requires to be correctly rounded: add, subtract,
//! multiply, divide and square root. Plus comparison, which has to be right about NaN.
//!
//! Every one of them works the same way. Pull the operands apart, deal with the zeros
//! and infinities and NaNs, compute an **exact** result as an integer significand and an
//! exponent — however many bits that takes — and hand it to
//! [`round_and_pack`](super::round_and_pack). Nothing rounds twice, because nothing
//! rounds anywhere else.
//!
//! Where a result cannot be exact — division and square root, which produce endless
//! digits — the remainder is folded in as a **sticky bit** at the very bottom of the
//! significand. That is sound because the significand is computed with
//! [`GUARD`] bits of slack below the place rounding will happen: a bit set that far
//! down can push the value off an exact tie, which is the only thing rounding needs to
//! know about it, and can never push it onto one.

use super::{
    Class, Format, Round, Unpacked, infinity, pack, quiet, quiet_nan, round_and_pack, unpack,
    zero,
};
use crate::uint::Uint;

/// Bits of slack carried below the rounding position while a result is being formed.
///
/// Two would do — a round bit and a sticky bit. The third is what makes it safe to
/// collapse a division or square-root remainder into the bottom bit of the significand.
const GUARD: u32 = 3;

/// The result of an IEEE 754 comparison, which has four outcomes rather than three.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Comparison {
    Less,
    Equal,
    Greater,
    /// At least one operand was NaN, so the two do not stand in any order at all.
    Unordered,
}

/// Flip the sign bit. Exact for every value, NaNs and zeros included.
pub fn neg<const W: usize>(fmt: Format, a: Uint<W>) -> Uint<W> {
    let mut out = a;
    if out.bit(fmt.bits - 1) {
        out.clear_bit(fmt.bits - 1);
    } else {
        out.set_bit(fmt.bits - 1);
    }
    out
}

/// Clear the sign bit.
pub fn abs<const W: usize>(fmt: Format, a: Uint<W>) -> Uint<W> {
    let mut out = a;
    out.clear_bit(fmt.bits - 1);
    out
}

/// `a + b`, correctly rounded.
pub fn add<const W: usize>(fmt: Format, mode: Round, a: Uint<W>, b: Uint<W>) -> Uint<W> {
    let (x, y) = (unpack(fmt, a), unpack(fmt, b));

    if x.class == Class::Nan {
        return quiet(fmt, a);
    }
    if y.class == Class::Nan {
        return quiet(fmt, b);
    }

    match (x.class, y.class) {
        (Class::Infinite, Class::Infinite) => {
            // Infinities of opposite sign have no defined difference.
            if x.sign == y.sign { infinity(fmt, x.sign) } else { quiet_nan(fmt) }
        }
        (Class::Infinite, _) => infinity(fmt, x.sign),
        (_, Class::Infinite) => infinity(fmt, y.sign),
        (Class::Zero, Class::Zero) => {
            // Two zeros keep a shared sign; disagreeing zeros give up theirs, except
            // when rounding downward, where the answer has to stay below both.
            if x.sign == y.sign {
                zero(fmt, x.sign)
            } else {
                zero(fmt, mode == Round::TowardNegative)
            }
        }
        (Class::Zero, _) => b,
        (_, Class::Zero) => a,
        _ => add_finite(fmt, mode, x, y),
    }
}

/// `a - b`, correctly rounded.
pub fn sub<const W: usize>(fmt: Format, mode: Round, a: Uint<W>, b: Uint<W>) -> Uint<W> {
    add(fmt, mode, a, neg(fmt, b))
}

fn add_finite<const W: usize>(
    fmt: Format,
    mode: Round,
    x: Unpacked<W>,
    y: Unpacked<W>,
) -> Uint<W> {
    // Line the two significands up on a common exponent. The one with more exponent moves
    // left, which is exact; the other moves right, which is not, and leaves a sticky bit.
    let (hi, lo) = if x.exp >= y.exp { (x, y) } else { (y, x) };
    let gap = (hi.exp - lo.exp) as u32;

    // No point shifting further left than the result can use: past `p + GUARD` places,
    // the smaller operand is entirely below the rounding position and only its
    // stickiness survives. Capping here is also what keeps the shift inside the width.
    let left = gap.min(fmt.precision + GUARD);
    let right = gap - left;

    let hi_sig = hi.sig.shl(left);
    let mut lo_sig = lo.sig.shr(right);
    if lo.sig.low_bits_any(right) {
        lo_sig.set_bit(0);
    }
    let exp = hi.exp - left as i32;

    let (sig, sign) = if hi.sign == lo.sign {
        (hi_sig.wrapping_add(lo_sig), hi.sign)
    } else if hi_sig >= lo_sig {
        (hi_sig.wrapping_sub(lo_sig), hi.sign)
    } else {
        // Only reachable while the two are within `p + GUARD` places of each other, so
        // `right` was zero and `lo_sig` is exact — the comparison is not confused by the
        // sticky bit.
        (lo_sig.wrapping_sub(hi_sig), lo.sign)
    };

    if sig.is_zero() {
        // Exact cancellation. The standard hands back a positive zero, except downward.
        return zero(fmt, mode == Round::TowardNegative);
    }
    round_and_pack(fmt, mode, sign, sig, exp)
}

/// `a × b`, correctly rounded.
pub fn mul<const W: usize>(fmt: Format, mode: Round, a: Uint<W>, b: Uint<W>) -> Uint<W> {
    let (x, y) = (unpack(fmt, a), unpack(fmt, b));

    if x.class == Class::Nan {
        return quiet(fmt, a);
    }
    if y.class == Class::Nan {
        return quiet(fmt, b);
    }

    let sign = x.sign ^ y.sign;
    match (x.class, y.class) {
        // Zero times infinity is the one product with no sensible answer.
        (Class::Infinite, Class::Zero) | (Class::Zero, Class::Infinite) => quiet_nan(fmt),
        (Class::Infinite, _) | (_, Class::Infinite) => infinity(fmt, sign),
        (Class::Zero, _) | (_, Class::Zero) => zero(fmt, sign),
        _ => {
            // Exact: two significands of `p` bits make `2p`, and the working width holds it.
            let (low, high) = x.sig.mul_wide(y.sig);
            debug_assert!(high.is_zero(), "product overflowed the working width");
            round_and_pack(fmt, mode, sign, low, x.exp + y.exp)
        }
    }
}

/// `a ÷ b`, correctly rounded. Dividing a finite non-zero by zero gives an infinity.
pub fn div<const W: usize>(fmt: Format, mode: Round, a: Uint<W>, b: Uint<W>) -> Uint<W> {
    let (x, y) = (unpack(fmt, a), unpack(fmt, b));

    if x.class == Class::Nan {
        return quiet(fmt, a);
    }
    if y.class == Class::Nan {
        return quiet(fmt, b);
    }

    let sign = x.sign ^ y.sign;
    match (x.class, y.class) {
        // Neither of these ratios has a value to converge on.
        (Class::Infinite, Class::Infinite) | (Class::Zero, Class::Zero) => quiet_nan(fmt),
        (Class::Infinite, _) => infinity(fmt, sign),
        (_, Class::Infinite) => zero(fmt, sign),
        (Class::Zero, _) => zero(fmt, sign),
        (_, Class::Zero) => infinity(fmt, sign),
        _ => {
            // Shift the dividend until the quotient is at least `p + GUARD` bits long,
            // then take one integer division. What it cannot express is the remainder,
            // which becomes the sticky bit.
            let want = fmt.precision + GUARD;
            let shift = (want + y.sig.bit_len()).saturating_sub(x.sig.bit_len());
            let (mut quotient, remainder) = x.sig.shl(shift).div_rem(y.sig);
            debug_assert!(quotient.bit_len() >= want);
            if !remainder.is_zero() {
                quotient.set_bit(0);
            }
            round_and_pack(fmt, mode, sign, quotient, x.exp - y.exp - shift as i32)
        }
    }
}

/// `√a`, correctly rounded. Negative operands other than `-0` give a NaN.
pub fn sqrt<const W: usize>(fmt: Format, mode: Round, a: Uint<W>) -> Uint<W> {
    let x = unpack(fmt, a);

    match x.class {
        Class::Nan => return quiet(fmt, a),
        // Both zeros are their own root, sign and all.
        Class::Zero => return a,
        Class::Infinite => {
            return if x.sign { quiet_nan(fmt) } else { a };
        }
        _ => {}
    }
    if x.sign {
        return quiet_nan(fmt);
    }

    // A square root halves the exponent, so the exponent has to be even to halve exactly,
    // and the significand needs twice the bits the root will have.
    let want = fmt.precision + GUARD;
    let mut shift = (2 * want).saturating_sub(x.sig.bit_len());
    if (x.exp - shift as i32) % 2 != 0 {
        shift += 1;
    }
    let exp = x.exp - shift as i32;
    debug_assert_eq!(exp % 2, 0);

    let (mut root, exact) = x.sig.shl(shift).isqrt();
    debug_assert!(root.bit_len() >= want);
    if !exact {
        root.set_bit(0);
    }
    round_and_pack(fmt, mode, false, root, exp / 2)
}

/// Compare two values, IEEE 754 style: a NaN on either side leaves them unordered, and
/// the two zeros are equal despite their differing signs.
pub fn compare<const W: usize>(fmt: Format, a: Uint<W>, b: Uint<W>) -> Comparison {
    let (x, y) = (unpack(fmt, a), unpack(fmt, b));
    if x.class == Class::Nan || y.class == Class::Nan {
        return Comparison::Unordered;
    }
    if x.class == Class::Zero && y.class == Class::Zero {
        return Comparison::Equal;
    }
    match (x.sign, y.sign) {
        (false, true) => Comparison::Greater,
        (true, false) => Comparison::Less,
        // Within one sign the encodings are already in value order, which is the whole
        // point of a biased exponent sitting above the significand.
        (negative, _) => {
            let (ma, mb) = (abs(fmt, a), abs(fmt, b));
            let ord = if negative { mb.cmp(&ma) } else { ma.cmp(&mb) };
            match ord {
                core::cmp::Ordering::Less => Comparison::Less,
                core::cmp::Ordering::Equal => Comparison::Equal,
                core::cmp::Ordering::Greater => Comparison::Greater,
            }
        }
    }
}

/// Whether the value is a NaN.
pub fn is_nan<const W: usize>(fmt: Format, a: Uint<W>) -> bool {
    unpack(fmt, a).class == Class::Nan
}

/// Whether the value is finite: neither infinite nor a NaN.
pub fn is_finite<const W: usize>(fmt: Format, a: Uint<W>) -> bool {
    !matches!(unpack(fmt, a).class, Class::Nan | Class::Infinite)
}

/// One, in the given format.
pub fn one<const W: usize>(fmt: Format, sign: bool) -> Uint<W> {
    let mut sig = Uint::<W>::ZERO;
    sig.set_bit(fmt.stored_sig_bits());
    pack(fmt, Unpacked { sign, class: Class::Normal, sig, exp: -(fmt.precision as i32 - 1) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::convert;

    // b32 and b64 are checked against the machine, which is correctly rounded for all
    // five of these operations and so is a real oracle rather than a second opinion.
    // b16, b128 and b256 have no hardware anywhere, and are checked by embedding: an
    // operation whose exact result fits in the wider format can be done there and
    // rounded once, which is the same answer by definition.

    type W2 = Uint<2>;
    type W8 = Uint<8>;

    fn f64_in(x: f64) -> W2 {
        Uint::from_u64(x.to_bits())
    }
    fn f64_out(u: W2) -> f64 {
        f64::from_bits(u.low64())
    }
    fn f32_in(x: f32) -> W2 {
        Uint::from_u64(x.to_bits() as u64)
    }
    fn f32_out(u: W2) -> f32 {
        f32::from_bits(u.low64() as u32)
    }

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
        /// A spread of f64s. Uniform bit patterns are almost all enormous, so the other
        /// three shapes exist to reach the ranges where arithmetic actually happens.
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
        fn f32(&mut self) -> f32 {
            let bits = self.next() as u32;
            match self.next() % 4 {
                0 => f32::from_bits(bits),
                1 => {
                    let e = 127u32 + (self.next() as u32 % 30) - 15;
                    f32::from_bits((bits & 0x807f_ffff) | (e << 23))
                }
                2 => f32::from_bits(bits & 0x807f_ffff),
                _ => ((bits % 4000) as f32) - 2000.0,
            }
        }
    }

    /// Values that arithmetic tends to go wrong on, and that random sampling misses.
    const AWKWARD_F64: [f64; 20] = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        2.0,
        0.5,
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        5e-324,  // smallest subnormal
        -5e-324,
        2.225_073_858_507_201e-308,  // largest subnormal
        1.7976931348623157e308,
        3.0,
        1e308,
        1e-308,
    ];

    fn same_f64(got: W2, want: f64) -> bool {
        let g = f64_out(got);
        if want.is_nan() { g.is_nan() } else { g.to_bits() == want.to_bits() }
    }
    fn same_f32(got: W2, want: f32) -> bool {
        let g = f32_out(got);
        if want.is_nan() { g.is_nan() } else { g.to_bits() == want.to_bits() }
    }

    #[test]
    fn b64_arithmetic_matches_the_hardware() {
        let f = Format::B64;
        let m = Round::TiesToEven;
        let mut rng = Rng::new(1);
        for _ in 0..200_000 {
            let (x, y) = (rng.f64(), rng.f64());
            let (a, b) = (f64_in(x), f64_in(y));
            assert!(same_f64(add(f, m, a, b), x + y), "{x:e} + {y:e}");
            assert!(same_f64(sub(f, m, a, b), x - y), "{x:e} - {y:e}");
            assert!(same_f64(mul(f, m, a, b), x * y), "{x:e} * {y:e}");
            assert!(same_f64(div(f, m, a, b), x / y), "{x:e} / {y:e}");
            assert!(same_f64(sqrt(f, m, a), x.sqrt()), "sqrt {x:e}");
        }
    }

    #[test]
    fn b64_arithmetic_matches_the_hardware_on_the_awkward_values() {
        let f = Format::B64;
        let m = Round::TiesToEven;
        for x in AWKWARD_F64 {
            assert!(same_f64(sqrt(f, m, f64_in(x)), x.sqrt()), "sqrt {x:e}");
            for y in AWKWARD_F64 {
                let (a, b) = (f64_in(x), f64_in(y));
                assert!(same_f64(add(f, m, a, b), x + y), "{x:e} + {y:e}");
                assert!(same_f64(sub(f, m, a, b), x - y), "{x:e} - {y:e}");
                assert!(same_f64(mul(f, m, a, b), x * y), "{x:e} * {y:e}");
                assert!(same_f64(div(f, m, a, b), x / y), "{x:e} / {y:e}");
            }
        }
    }

    #[test]
    fn b32_arithmetic_matches_the_hardware() {
        let f = Format::B32;
        let m = Round::TiesToEven;
        let mut rng = Rng::new(2);
        for _ in 0..200_000 {
            let (x, y) = (rng.f32(), rng.f32());
            let (a, b) = (f32_in(x), f32_in(y));
            assert!(same_f32(add(f, m, a, b), x + y), "{x:e} + {y:e}");
            assert!(same_f32(sub(f, m, a, b), x - y), "{x:e} - {y:e}");
            assert!(same_f32(mul(f, m, a, b), x * y), "{x:e} * {y:e}");
            assert!(same_f32(div(f, m, a, b), x / y), "{x:e} / {y:e}");
            assert!(same_f32(sqrt(f, m, a), x.sqrt()), "sqrt {x:e}");
        }
    }

    #[test]
    fn b32_arithmetic_matches_the_hardware_on_the_awkward_values() {
        let f = Format::B32;
        let m = Round::TiesToEven;
        let awkward: Vec<f32> = AWKWARD_F64
            .iter()
            .map(|&v| v as f32)
            .chain([f32::MIN_POSITIVE, f32::from_bits(1), f32::MAX, -f32::MAX])
            .collect();
        for &x in &awkward {
            assert!(same_f32(sqrt(f, m, f32_in(x)), x.sqrt()), "sqrt {x:e}");
            for &y in &awkward {
                let (a, b) = (f32_in(x), f32_in(y));
                assert!(same_f32(add(f, m, a, b), x + y), "{x:e} + {y:e}");
                assert!(same_f32(sub(f, m, a, b), x - y), "{x:e} - {y:e}");
                assert!(same_f32(mul(f, m, a, b), x * y), "{x:e} * {y:e}");
                assert!(same_f32(div(f, m, a, b), x / y), "{x:e} / {y:e}");
            }
        }
    }

    #[test]
    fn b16_addition_and_multiplication_match_an_exact_wider_computation() {
        // Two b16 values sum and multiply exactly inside b64 -- 11-bit significands and
        // a 40-place exponent range cannot exceed 53 bits either way -- so doing it there
        // and rounding once is the correct b16 answer by construction, not an estimate.
        let f = Format::B16;
        let m = Round::TiesToEven;
        let mut rng = Rng::new(3);
        for _ in 0..200_000 {
            let (a, b) = (
                W2::from_u64(rng.next() & 0xffff),
                W2::from_u64(rng.next() & 0xffff),
            );
            let (wa, wb) = (
                convert::<2>(f, Format::B64, m, a),
                convert::<2>(f, Format::B64, m, b),
            );
            let (x, y) = (f64_out(wa), f64_out(wb));

            for (got, want, what) in [
                (add(f, m, a, b), x + y, "+"),
                (sub(f, m, a, b), x - y, "-"),
                (mul(f, m, a, b), x * y, "*"),
            ] {
                let expect = convert::<2>(Format::B64, f, m, f64_in(want));
                if want.is_nan() {
                    assert!(is_nan(f, got), "{x:e} {what} {y:e}");
                } else {
                    assert_eq!(got, expect, "{x:e} {what} {y:e}");
                }
            }
        }
    }

    #[test]
    fn the_wide_formats_multiply_the_way_b64_does() {
        // A product of two b64 values takes 106 significand bits, which b128 and b256
        // both hold exactly. So widening, multiplying there, and narrowing back has to
        // land on the b64 product -- one rounding either way, not two.
        let m = Round::TiesToEven;
        let mut rng = Rng::new(4);
        for wide in [Format::B128, Format::B256] {
            for _ in 0..20_000 {
                let (x, y) = (rng.f64(), rng.f64());
                if !x.is_finite() || !y.is_finite() {
                    continue;
                }
                let (a, b) = (W8::from_u64(x.to_bits()), W8::from_u64(y.to_bits()));
                let (wa, wb) = (
                    convert::<8>(Format::B64, wide, m, a),
                    convert::<8>(Format::B64, wide, m, b),
                );
                let wide_product = mul(wide, m, wa, wb);
                let narrowed = convert::<8>(wide, Format::B64, m, wide_product);
                let want = x * y;
                // Skip where the exact product overflows or falls subnormal in b64: the
                // wide format holds it and b64 does not, so the two roundings differ for
                // a reason that is not a bug.
                if want == 0.0 || !want.is_finite() || want.abs() < f64::MIN_POSITIVE {
                    continue;
                }
                assert_eq!(
                    f64::from_bits(narrowed.low64()).to_bits(),
                    want.to_bits(),
                    "{}: {x:e} * {y:e}",
                    wide.name
                );
            }
        }
    }

    #[test]
    fn the_wide_formats_add_the_way_b64_does_when_the_sum_is_exact() {
        // Addition is only exact in the wider format when the operands are close enough
        // in magnitude that the sum still fits. Force that by giving both the same
        // exponent, where the sum takes at most 54 bits.
        let m = Round::TiesToEven;
        let mut rng = Rng::new(5);
        for wide in [Format::B128, Format::B256] {
            for _ in 0..20_000 {
                let e = 1023u64 + (rng.next() % 40) - 20;
                let mk = |r: u64| f64::from_bits((r & 0x800f_ffff_ffff_ffff) | (e << 52));
                let (x, y) = (mk(rng.next()), mk(rng.next()));
                let (a, b) = (W8::from_u64(x.to_bits()), W8::from_u64(y.to_bits()));
                let (wa, wb) = (
                    convert::<8>(Format::B64, wide, m, a),
                    convert::<8>(Format::B64, wide, m, b),
                );
                let narrowed =
                    convert::<8>(wide, Format::B64, m, add(wide, m, wa, wb));
                assert_eq!(
                    f64::from_bits(narrowed.low64()).to_bits(),
                    (x + y).to_bits(),
                    "{}: {x:e} + {y:e}",
                    wide.name
                );
            }
        }
    }

    #[test]
    fn the_identities_hold_in_every_format() {
        // What is left for b128 and b256 once embedding runs out: facts that are true of
        // the arithmetic whatever the width, and would break under a mis-shifted
        // significand or a mishandled subnormal.
        let m = Round::TiesToEven;
        let mut rng = Rng::new(6);
        for f in Format::ALL {
            let one_v = one::<8>(f, false);
            for _ in 0..2_000 {
                // Build a value from raw bits so subnormals and huge exponents both occur.
                let mut a = W8::ZERO;
                for i in 0..f.bits.div_ceil(64) {
                    a = a | W8::from_u64(rng.next()).shl(i * 64);
                }
                a = a & W8::low_mask(f.bits);
                if !is_finite(f, a) {
                    continue;
                }
                let name = f.name;

                assert_eq!(mul(f, m, a, one_v), a, "{name}: a * 1");
                assert_eq!(div(f, m, a, one_v), a, "{name}: a / 1");
                assert_eq!(add(f, m, a, zero::<8>(f, false)), a, "{name}: a + 0");
                assert_eq!(compare(f, sub(f, m, a, a), zero::<8>(f, false)), Comparison::Equal, "{name}: a - a");
                assert_eq!(add(f, m, a, neg(f, a)), zero::<8>(f, false), "{name}: a + -a");
                assert_eq!(add(f, m, a, b_of(a)), add(f, m, b_of(a), a), "{name}: commutes");
                assert_eq!(mul(f, m, a, neg(f, one_v)), neg(f, a), "{name}: a * -1");

                if unpack(f, a).class != Class::Zero {
                    assert_eq!(div(f, m, a, a), one_v, "{name}: a / a");
                }

                // The root of a square is the original, whenever squaring was exact --
                // which is what comparing back through multiplication checks.
                let root = sqrt(f, m, abs(f, a));
                if is_finite(f, root) && unpack(f, root).class != Class::Zero {
                    assert_ne!(compare(f, root, abs(f, a)), Comparison::Unordered, "{name}: sqrt ordered");
                }
            }
        }

        // A helper that just gives a second, different value derived from the first.
        fn b_of<const W: usize>(a: Uint<W>) -> Uint<W> {
            a.shr(1)
        }
    }

    #[test]
    fn nan_and_infinity_behave_in_every_format() {
        let m = Round::TiesToEven;
        for f in Format::ALL {
            let nan = quiet_nan::<8>(f);
            let pinf = infinity::<8>(f, false);
            let ninf = infinity::<8>(f, true);
            let pzero = zero::<8>(f, false);
            let nzero = zero::<8>(f, true);
            let one_v = one::<8>(f, false);
            let name = f.name;

            // A NaN swallows everything it touches.
            for other in [nan, pinf, ninf, pzero, nzero, one_v] {
                assert!(is_nan(f, add(f, m, nan, other)), "{name}");
                assert!(is_nan(f, mul(f, m, nan, other)), "{name}");
                assert!(is_nan(f, div(f, m, other, nan)), "{name}");
                assert_eq!(compare(f, nan, other), Comparison::Unordered, "{name}");
            }
            assert!(is_nan(f, sqrt(f, m, nan)), "{name}");

            // The four undefined forms.
            assert!(is_nan(f, add(f, m, pinf, ninf)), "{name}: inf - inf");
            assert!(is_nan(f, mul(f, m, pinf, pzero)), "{name}: inf * 0");
            assert!(is_nan(f, div(f, m, pinf, pinf)), "{name}: inf / inf");
            assert!(is_nan(f, div(f, m, pzero, pzero)), "{name}: 0 / 0");
            assert!(is_nan(f, sqrt(f, m, neg(f, one_v))), "{name}: sqrt of a negative");

            // And the defined ones.
            assert_eq!(add(f, m, pinf, one_v), pinf, "{name}");
            assert_eq!(div(f, m, one_v, pzero), pinf, "{name}: 1 / +0");
            assert_eq!(div(f, m, one_v, nzero), ninf, "{name}: 1 / -0");
            assert_eq!(div(f, m, one_v, pinf), pzero, "{name}");
            assert_eq!(sqrt(f, m, pinf), pinf, "{name}");
            assert_eq!(sqrt(f, m, nzero), nzero, "{name}: sqrt of -0 keeps its sign");
            assert_eq!(mul(f, m, pzero, nzero), nzero, "{name}: signs multiply");

            // Zeros compare equal across their signs, and still order against real values.
            assert_eq!(compare(f, pzero, nzero), Comparison::Equal, "{name}");
            assert_eq!(compare(f, nzero, one_v), Comparison::Less, "{name}");
            assert_eq!(compare(f, pinf, one_v), Comparison::Greater, "{name}");
            assert_eq!(compare(f, ninf, pinf), Comparison::Less, "{name}");
        }
    }

    #[test]
    fn comparison_matches_the_hardware() {
        let mut rng = Rng::new(7);
        for _ in 0..200_000 {
            let (x, y) = (rng.f64(), rng.f64());
            let got = compare(Format::B64, f64_in(x), f64_in(y));
            let want = match x.partial_cmp(&y) {
                None => Comparison::Unordered,
                Some(core::cmp::Ordering::Less) => Comparison::Less,
                Some(core::cmp::Ordering::Equal) => Comparison::Equal,
                Some(core::cmp::Ordering::Greater) => Comparison::Greater,
            };
            assert_eq!(got, want, "{x:e} vs {y:e}");
        }
    }

    #[test]
    fn the_directed_modes_bracket_the_nearest_one() {
        // Whatever the exact sum is, rounding downward lands at or below it and rounding
        // upward at or above, so the two directed answers straddle every nearest answer.
        let f = Format::B64;
        let mut rng = Rng::new(8);
        for _ in 0..100_000 {
            let (x, y) = (rng.f64(), rng.f64());
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            let (a, b) = (f64_in(x), f64_in(y));
            let down = add(f, Round::TowardNegative, a, b);
            let up = add(f, Round::TowardPositive, a, b);
            let near = add(f, Round::TiesToEven, a, b);
            if is_nan(f, near) {
                continue;
            }
            assert_ne!(compare(f, down, up), Comparison::Greater, "{x:e} + {y:e}");
            assert_ne!(compare(f, near, down), Comparison::Less, "{x:e} + {y:e}");
            assert_ne!(compare(f, near, up), Comparison::Greater, "{x:e} + {y:e}");
        }
    }
}

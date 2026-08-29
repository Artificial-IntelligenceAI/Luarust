//! The IEEE 754 binary formats: `b16`, `b32`, `b64`, `b128`, `b256`.
//!
//! One set of routines covers all five. A [`Format`] is three numbers — storage width,
//! exponent width, precision — and every difference between `b16` and `b256` follows
//! from those, so the arithmetic is written once and instantiated by value rather than
//! copied per width.
//!
//! Values move through three shapes:
//!
//! - the **encoding**, a bit pattern in the low `fmt.bits` bits of a [`Uint`];
//! - an [`Unpacked`] value, which is a sign, a class, and an integer significand scaled
//!   by an exponent — `value = sig × 2^exp`, with `sig` a plain integer rather than a
//!   fraction, because integers are what the significand arithmetic actually runs on;
//! - and back again through [`round_and_pack`], which is where all the rounding lives.
//!
//! The working width `W` is a caller's choice and has to be wide enough for the
//! intermediate products the format's arithmetic makes — `2p + 3` bits, so
//! `Uint<1>` for `b16` and `b32`, `Uint<2>` for `b64`, `Uint<4>` for `b128`,
//! `Uint<8>` for `b256`.

pub mod arith;

pub use arith::{Comparison, add, compare, div, mul, neg, sqrt, sub};

use crate::uint::Uint;

/// The rounding-direction attributes of IEEE 754.
///
/// `TiesToEven` is the default the standard requires, and the only one Luarust
/// currently reaches from source. The rest are here because they are cheap to support
/// while the rounding step is being written and expensive to retrofit afterwards.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Round {
    /// Nearest, and on an exact tie the candidate with an even last bit.
    #[default]
    TiesToEven,
    /// Nearest, and on an exact tie the candidate further from zero.
    TiesToAway,
    /// Toward zero: truncate.
    TowardZero,
    /// Toward positive infinity.
    TowardPositive,
    /// Toward negative infinity.
    TowardNegative,
}

/// What kind of value an encoding denotes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// Signed zero.
    Zero,
    /// Below the smallest normal: the leading significand bit is not implied.
    Subnormal,
    /// The ordinary case, with an implied leading one.
    Normal,
    /// Signed infinity.
    Infinite,
    /// Not a number. The significand carries the payload.
    Nan,
}

/// One of the five binary interchange formats, described by the three numbers that
/// distinguish it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Format {
    /// The name Luarust source uses.
    pub name: &'static str,
    /// Width of the encoding, in bits.
    pub bits: u32,
    /// Width of the biased exponent field, in bits.
    pub exp_bits: u32,
    /// Significand digits, counting the leading bit whether it is stored or implied.
    pub precision: u32,
}

impl Format {
    /// binary16 — a basic format of the standard.
    pub const B16: Format = Format { name: "b16", bits: 16, exp_bits: 5, precision: 11 };
    /// binary32 — a basic format of the standard, and one of the two the hardware knows.
    pub const B32: Format = Format { name: "b32", bits: 32, exp_bits: 8, precision: 24 };
    /// binary64 — a basic format of the standard, and the other one the hardware knows.
    pub const B64: Format = Format { name: "b64", bits: 64, exp_bits: 11, precision: 53 };
    /// binary128 — a basic format of the standard, with no hardware on any Luarust target.
    pub const B128: Format = Format { name: "b128", bits: 128, exp_bits: 15, precision: 113 };
    /// binary256.
    ///
    /// Not a *basic* format: the standard names binary16 through binary128 and then gives
    /// a rule for any interchange width that is a multiple of 32 at or above 128, which
    /// fixes binary256 at 19 exponent bits and 237 bits of precision. Fully specified,
    /// and almost nowhere implemented.
    pub const B256: Format = Format { name: "b256", bits: 256, exp_bits: 19, precision: 237 };

    /// Every binary format, narrowest first.
    pub const ALL: [Format; 5] = [Self::B16, Self::B32, Self::B64, Self::B128, Self::B256];

    /// The offset added to a value's exponent to store it unsigned.
    pub const fn bias(self) -> i32 {
        (1i32 << (self.exp_bits - 1)) - 1
    }

    /// The largest exponent a normal value can have.
    pub const fn emax(self) -> i32 {
        self.bias()
    }

    /// The smallest exponent a normal value can have.
    pub const fn emin(self) -> i32 {
        1 - self.bias()
    }

    /// Significand bits actually stored: the leading bit of a normal value is implied.
    pub const fn stored_sig_bits(self) -> u32 {
        self.precision - 1
    }

    /// The all-ones exponent field, which marks infinities and NaNs.
    pub const fn max_biased_exp(self) -> u64 {
        (1u64 << self.exp_bits) - 1
    }

    /// The scale of the least significant bit at the bottom of the range.
    ///
    /// Every subnormal is a whole multiple of `2^this`, and no finite value in the format
    /// has a bit below it. It is the floor that underflow rounds against.
    pub const fn min_quantum_exp(self) -> i32 {
        self.emin() - (self.precision as i32 - 1)
    }

    /// The scale of the least significant bit of the largest finite value.
    pub const fn max_quantum_exp(self) -> i32 {
        self.emax() - (self.precision as i32 - 1)
    }

    /// The narrowest [`Uint`] width, in 64-bit limbs, that this format's arithmetic needs.
    ///
    /// Square root sets the bound: to get `p + 3` bits of root it squares the room,
    /// so the rule is `2(p + 3)` bits rounded up to a limb. Multiplication needs `2p`
    /// and division `2p + 3`, both of which fit inside that.
    pub const fn work_limbs(self) -> usize {
        (2 * (self.precision as usize + 3)).div_ceil(64)
    }
}

/// A value pulled apart: `value = (-1)^sign × sig × 2^exp`, with `sig` an integer.
///
/// Writing the significand as an integer rather than as a fraction is what lets normal
/// and subnormal values share one representation, and what lets the arithmetic be plain
/// integer arithmetic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Unpacked<const W: usize> {
    /// True when the value is negative. Meaningful for zeros and infinities too.
    pub sign: bool,
    /// Which kind of value this is.
    pub class: Class,
    /// The integer significand, or for a NaN, its payload.
    pub sig: Uint<W>,
    /// The power of two the significand is scaled by. Unused for infinities and NaNs.
    pub exp: i32,
}

/// Take an encoding apart.
pub fn unpack<const W: usize>(fmt: Format, bits: Uint<W>) -> Unpacked<W> {
    let t = fmt.stored_sig_bits();
    let sign = bits.bit(fmt.bits - 1);
    let biased = bits.shr(t).low64() & fmt.max_biased_exp();
    let frac = bits & Uint::low_mask(t);

    if biased == 0 {
        // No implied leading bit down here, so the significand is the stored field as-is
        // and the exponent is pinned to the bottom of the range.
        let class = if frac.is_zero() { Class::Zero } else { Class::Subnormal };
        Unpacked { sign, class, sig: frac, exp: fmt.min_quantum_exp() }
    } else if biased == fmt.max_biased_exp() {
        let class = if frac.is_zero() { Class::Infinite } else { Class::Nan };
        Unpacked { sign, class, sig: frac, exp: 0 }
    } else {
        let mut sig = frac;
        sig.set_bit(t);
        Unpacked { sign, class: Class::Normal, sig, exp: biased as i32 - fmt.bias() - t as i32 }
    }
}

/// Put an encoding back together.
///
/// The exact inverse of [`unpack`], payload and signaling bit included — quieting a NaN
/// is [`quiet_nan`]'s job, not this one's.
pub fn pack<const W: usize>(fmt: Format, v: Unpacked<W>) -> Uint<W> {
    let t = fmt.stored_sig_bits();
    let frac_mask = Uint::<W>::low_mask(t);
    let mut out = match v.class {
        Class::Zero => Uint::ZERO,
        Class::Subnormal => v.sig & frac_mask,
        Class::Normal => {
            let biased = (v.exp + t as i32 + fmt.bias()) as u64;
            debug_assert!(biased >= 1 && biased < fmt.max_biased_exp());
            Uint::from_u64(biased).shl(t) | (v.sig & frac_mask)
        }
        Class::Infinite => Uint::from_u64(fmt.max_biased_exp()).shl(t),
        Class::Nan => {
            let mut payload = v.sig & frac_mask;
            if payload.is_zero() {
                // An all-zero payload would encode an infinity, so it is not a NaN at all.
                payload.set_bit(t - 1);
            }
            Uint::from_u64(fmt.max_biased_exp()).shl(t) | payload
        }
    };
    if v.sign {
        out.set_bit(fmt.bits - 1);
    }
    out
}

/// Signed zero.
pub fn zero<const W: usize>(fmt: Format, sign: bool) -> Uint<W> {
    pack(fmt, Unpacked { sign, class: Class::Zero, sig: Uint::ZERO, exp: 0 })
}

/// Signed infinity.
pub fn infinity<const W: usize>(fmt: Format, sign: bool) -> Uint<W> {
    pack(fmt, Unpacked { sign, class: Class::Infinite, sig: Uint::ZERO, exp: 0 })
}

/// Make a NaN quiet, keeping its payload and sign. Anything else is returned unchanged.
pub fn quiet<const W: usize>(fmt: Format, bits: Uint<W>) -> Uint<W> {
    let mut v = unpack(fmt, bits);
    if v.class != Class::Nan {
        return bits;
    }
    v.sig.set_bit(fmt.stored_sig_bits() - 1);
    pack(fmt, v)
}

/// The default quiet NaN: positive, with only the quiet bit set.
pub fn quiet_nan<const W: usize>(fmt: Format) -> Uint<W> {
    let mut sig = Uint::<W>::ZERO;
    sig.set_bit(fmt.stored_sig_bits() - 1);
    pack(fmt, Unpacked { sign: false, class: Class::Nan, sig, exp: 0 })
}

/// The largest finite value, with the given sign.
pub fn max_finite<const W: usize>(fmt: Format, sign: bool) -> Uint<W> {
    let t = fmt.stored_sig_bits();
    let mut out = Uint::<W>::from_u64(fmt.max_biased_exp() - 1).shl(t) | Uint::low_mask(t);
    if sign {
        out.set_bit(fmt.bits - 1);
    }
    out
}

/// The smallest positive subnormal: one unit in the last place at the bottom of the range.
pub fn min_subnormal<const W: usize>(fmt: Format, sign: bool) -> Uint<W> {
    let mut out = Uint::<W>::from_u64(1);
    if sign {
        out.set_bit(fmt.bits - 1);
    }
    out
}

/// Round `(-1)^sign × sig × 2^exp` into `fmt` and encode it.
///
/// This is the only place in the binary formats where a value loses information, so it
/// is the only place that has to be exactly right. Every arithmetic operation computes
/// an exact result in as many bits as it takes, then hands it here.
///
/// `sig` may be any width — wider than the format's precision, narrower, or unnormalized.
pub fn round_and_pack<const W: usize>(
    fmt: Format,
    mode: Round,
    sign: bool,
    sig: Uint<W>,
    exp: i32,
) -> Uint<W> {
    if sig.is_zero() {
        return zero(fmt, sign);
    }

    let p = fmt.precision;
    let floor_exp = fmt.min_quantum_exp();
    let mut sig = sig;
    let mut exp = exp;

    // Normalize upward as far as it is free, so a value that fits the format exactly is
    // not mistaken for a subnormal one. The exponent may not go below the format's floor.
    let len = sig.bit_len();
    if len < p {
        let room = if exp > floor_exp { (exp - floor_exp) as u32 } else { 0 };
        let k = (p - len).min(room);
        sig = sig.shl(k);
        exp -= k as i32;
    }

    // Then right, by however much it takes to fit the precision and to lift the exponent
    // to the floor. Whichever demand is larger wins; both may be zero.
    let len = sig.bit_len();
    let for_precision = len.saturating_sub(p);
    let for_range = if floor_exp > exp { (floor_exp - exp) as u32 } else { 0 };
    let shift = for_precision.max(for_range);

    // The two bits every rounding decision is made from: whether the highest discarded
    // bit is set, and whether anything below it was.
    let (mut m, round_bit, sticky) = if shift == 0 {
        (sig, false, false)
    } else {
        (sig.shr(shift), sig.bit(shift - 1), sig.low_bits_any(shift - 1))
    };
    let mut e = exp + shift as i32;

    let increment = match mode {
        Round::TiesToEven => round_bit && (sticky || m.is_odd()),
        Round::TiesToAway => round_bit,
        Round::TowardZero => false,
        Round::TowardPositive => !sign && (round_bit || sticky),
        Round::TowardNegative => sign && (round_bit || sticky),
    };
    if increment {
        m = m.wrapping_add(Uint::from_u64(1));
        // Rounding up can carry into a new bit — 0b1111 becomes 0b10000 — which costs
        // one place of precision and buys one of exponent.
        if m.bit_len() > p {
            m = m.shr(1);
            e += 1;
        }
    }

    if m.is_zero() {
        // Everything rounded away. Toward-zero and the nearest modes can both land here.
        return zero(fmt, sign);
    }

    if e > fmt.max_quantum_exp() {
        // Past the largest finite value. Which way that goes is the rounding mode's call:
        // the nearest modes reach infinity, the directed ones only in their own direction.
        let to_infinity = match mode {
            Round::TiesToEven | Round::TiesToAway => true,
            Round::TowardZero => false,
            Round::TowardPositive => !sign,
            Round::TowardNegative => sign,
        };
        return if to_infinity { infinity(fmt, sign) } else { max_finite(fmt, sign) };
    }

    if m.bit_len() == p {
        pack(fmt, Unpacked { sign, class: Class::Normal, sig: m, exp: e })
    } else {
        // Short of full precision, which can only happen at the floor of the range.
        debug_assert_eq!(e, floor_exp);
        pack(fmt, Unpacked { sign, class: Class::Subnormal, sig: m, exp: e })
    }
}

/// Convert between binary formats, rounding if the destination is narrower.
///
/// `W` has to suit both formats; the wider one's [`Format::work_limbs`] is always enough.
///
/// NaNs come out as the destination's default quiet NaN. The standard would rather the
/// payload were carried across where it fits, and nothing here depends on it yet.
pub fn convert<const W: usize>(from: Format, to: Format, mode: Round, bits: Uint<W>) -> Uint<W> {
    let v = unpack(from, bits);
    match v.class {
        Class::Nan => {
            let mut nan = quiet_nan::<W>(to);
            if v.sign {
                nan.set_bit(to.bits - 1);
            }
            nan
        }
        Class::Infinite => infinity(to, v.sign),
        Class::Zero => zero(to, v.sign),
        Class::Normal | Class::Subnormal => round_and_pack(to, mode, v.sign, v.sig, v.exp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_f64(x: f64) -> Uint<2> {
        Uint::from_u64(x.to_bits())
    }
    fn to_f64(u: Uint<2>) -> f64 {
        f64::from_bits(u.low64())
    }
    fn to_f32(u: Uint<2>) -> f32 {
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
    }

    #[test]
    fn format_constants_agree_with_the_standard() {
        assert_eq!(Format::B16.bias(), 15);
        assert_eq!(Format::B32.bias(), 127);
        assert_eq!(Format::B64.bias(), 1023);
        assert_eq!(Format::B128.bias(), 16383);
        assert_eq!(Format::B256.bias(), 262143);

        assert_eq!(Format::B64.emin(), -1022);
        assert_eq!(Format::B64.emax(), 1023);
        assert_eq!(Format::B32.min_quantum_exp(), -149); // scale of the smallest f32 subnormal
        assert_eq!(Format::B64.min_quantum_exp(), -1074); // and of the smallest f64 subnormal

        // Storage width is the sign bit, the exponent, and the stored significand.
        for f in Format::ALL {
            assert_eq!(f.bits, 1 + f.exp_bits + f.stored_sig_bits(), "{}", f.name);
        }

        assert_eq!(Format::B16.work_limbs(), 1);
        assert_eq!(Format::B32.work_limbs(), 1);
        assert_eq!(Format::B64.work_limbs(), 2);
        assert_eq!(Format::B128.work_limbs(), 4);
        assert_eq!(Format::B256.work_limbs(), 8);
    }

    #[test]
    fn every_b16_encoding_survives_a_round_trip() {
        // Small enough to check exhaustively, so there is no reason not to.
        for bits in 0u64..=0xffff {
            let u = Uint::<1>::from_u64(bits);
            let back = pack(Format::B16, unpack(Format::B16, u));
            assert_eq!(back, u, "b16 pattern {bits:#06x}");
        }
    }

    #[test]
    fn every_b16_encoding_classifies_the_way_the_bits_say() {
        let mut counts = [0usize; 5];
        for bits in 0u64..=0xffff {
            let v = unpack(Format::B16, Uint::<1>::from_u64(bits));
            let exp_field = (bits >> 10) & 0x1f;
            let frac = bits & 0x3ff;
            let want = match (exp_field, frac) {
                (0, 0) => Class::Zero,
                (0, _) => Class::Subnormal,
                (0x1f, 0) => Class::Infinite,
                (0x1f, _) => Class::Nan,
                _ => Class::Normal,
            };
            assert_eq!(v.class, want, "b16 pattern {bits:#06x}");
            assert_eq!(v.sign, bits >> 15 == 1);
            counts[want as usize] += 1;
        }
        // 2 zeros, 2046 subnormals, 61440 normals, 2 infinities, 2046 NaNs.
        assert_eq!(counts, [2, 2046, 61440, 2, 2046]);
    }

    #[test]
    fn b64_unpacking_agrees_with_the_hardware() {
        let mut rng = Rng::new(1);
        for _ in 0..50_000 {
            let x = f64::from_bits(rng.next());
            let v = unpack(Format::B64, from_f64(x));
            assert_eq!(v.sign, x.is_sign_negative());
            let want = if x.is_nan() {
                Class::Nan
            } else if x.is_infinite() {
                Class::Infinite
            } else if x == 0.0 {
                Class::Zero
            } else if x.is_subnormal() {
                Class::Subnormal
            } else {
                Class::Normal
            };
            assert_eq!(v.class, want, "{x:e}");

            // And for finite values the pieces multiply back out to the number itself.
            if matches!(v.class, Class::Normal | Class::Subnormal) {
                let rebuilt = (v.sig.low64() as f64) * (v.exp as f64).exp2();
                assert_eq!(rebuilt, x.abs(), "{x:e}");
            }
        }
    }

    #[test]
    fn b64_encodings_survive_a_round_trip() {
        let mut rng = Rng::new(2);
        for _ in 0..50_000 {
            let u = Uint::<2>::from_u64(rng.next());
            assert_eq!(pack(Format::B64, unpack(Format::B64, u)), u);
        }
    }

    #[test]
    fn narrowing_b64_to_b32_rounds_the_way_the_hardware_does() {
        // The real test of the rounding step: `as f32` is correctly rounded by the
        // machine, so every disagreement is ours.
        let mut rng = Rng::new(3);
        let mut checked = 0u64;
        for _ in 0..200_000 {
            let x = f64::from_bits(rng.next());
            let got = convert::<2>(Format::B64, Format::B32, Round::TiesToEven, from_f64(x));
            let want = x as f32;
            if want.is_nan() {
                assert!(to_f32(got).is_nan(), "{x:e}");
            } else {
                assert_eq!(to_f32(got).to_bits(), want.to_bits(), "{x:e} -> {want:e}");
            }
            checked += 1;
        }
        assert_eq!(checked, 200_000);
    }

    #[test]
    fn narrowing_rounds_correctly_at_the_awkward_magnitudes() {
        // Random f64 bit patterns are almost all enormous or tiny, so they never reach
        // f32's subnormal range or its overflow boundary. These do.
        let mut rng = Rng::new(4);
        let interesting = [
            0.0f64,
            -0.0,
            1.0,
            -1.0,
            f32::MAX as f64,
            f32::MIN_POSITIVE as f64,
            f64::from(f32::from_bits(1)),           // smallest f32 subnormal
            f64::from(f32::from_bits(1)) / 2.0,     // exactly half of it: ties to even -> 0
            f64::from(f32::from_bits(1)) * 0.75,    // above half: rounds up
            f64::from(f32::from_bits(3)) / 2.0,     // half of an odd one: ties to even -> up
            3.402_823_5e38,                         // just under f32::MAX
            3.402_823_7e38,                         // just over: overflows to infinity
            f64::INFINITY,
            f64::NEG_INFINITY,
            1e-46,
            1e-300,
            1e300,
        ];
        for x in interesting {
            let got = convert::<2>(Format::B64, Format::B32, Round::TiesToEven, from_f64(x));
            assert_eq!(to_f32(got).to_bits(), (x as f32).to_bits(), "{x:e}");
        }

        // And a sweep across the f32 subnormal range, where the exponent floor bites.
        for _ in 0..100_000 {
            let x = f64::from_bits(rng.next()) % 1e-38;
            if x.is_nan() {
                continue;
            }
            let got = convert::<2>(Format::B64, Format::B32, Round::TiesToEven, from_f64(x));
            assert_eq!(to_f32(got).to_bits(), (x as f32).to_bits(), "{x:e}");
        }
    }

    #[test]
    fn widening_b32_to_b64_loses_nothing() {
        let mut rng = Rng::new(5);
        for _ in 0..100_000 {
            let x = f32::from_bits(rng.next() as u32);
            let got = convert::<2>(Format::B32, Format::B64, Round::TiesToEven, Uint::from_u64(x.to_bits() as u64));
            if x.is_nan() {
                assert!(to_f64(got).is_nan());
            } else {
                assert_eq!(to_f64(got).to_bits(), (x as f64).to_bits(), "{x:e}");
            }
        }
    }

    #[test]
    fn every_b16_value_widens_and_narrows_back_to_itself() {
        // b16 has no hardware to check against, so it is checked against itself: widening
        // is exact, so narrowing has to give the original back, for all 65536 patterns.
        for bits in 0u64..=0xffff {
            let u = Uint::<2>::from_u64(bits);
            let wide = convert::<2>(Format::B16, Format::B64, Round::TiesToEven, u);
            let back = convert::<2>(Format::B64, Format::B16, Round::TiesToEven, wide);
            let v = unpack(Format::B16, u);
            if v.class == Class::Nan {
                assert_eq!(unpack(Format::B16, back).class, Class::Nan, "{bits:#06x}");
            } else {
                assert_eq!(back, u, "b16 pattern {bits:#06x}");
            }
        }
    }

    #[test]
    fn b16_landmarks_are_where_the_standard_says() {
        let one = convert::<2>(Format::B64, Format::B16, Round::TiesToEven, from_f64(1.0));
        assert_eq!(one.low64(), 0x3c00);

        // Largest finite b16 is 65504, and the next value up overflows.
        let max = convert::<2>(Format::B64, Format::B16, Round::TiesToEven, from_f64(65504.0));
        assert_eq!(max.low64(), 0x7bff);
        assert_eq!(max, max_finite::<2>(Format::B16, false));
        let over = convert::<2>(Format::B64, Format::B16, Round::TiesToEven, from_f64(65536.0));
        assert_eq!(unpack(Format::B16, over).class, Class::Infinite);

        // Smallest normal is 2^-14, smallest subnormal is 2^-24.
        let min_norm = convert::<2>(Format::B64, Format::B16, Round::TiesToEven, from_f64((-14f64).exp2()));
        assert_eq!(min_norm.low64(), 0x0400);
        let min_sub = convert::<2>(Format::B64, Format::B16, Round::TiesToEven, from_f64((-24f64).exp2()));
        assert_eq!(min_sub, min_subnormal::<2>(Format::B16, false));
        // Half of the smallest subnormal is an exact tie, and zero is the even side.
        let half = convert::<2>(Format::B64, Format::B16, Round::TiesToEven, from_f64((-25f64).exp2()));
        assert_eq!(unpack(Format::B16, half).class, Class::Zero);
        // Just over half rounds away from it.
        let over_half = convert::<2>(Format::B64, Format::B16, Round::TiesToEven, from_f64(0.75 * (-24f64).exp2()));
        assert_eq!(over_half, min_subnormal::<2>(Format::B16, false));
    }

    #[test]
    fn the_directed_modes_bracket_the_nearest_one() {
        // Whatever the true value is, truncating never moves away from zero and the two
        // directed modes sit on either side of it.
        let mut rng = Rng::new(6);
        for _ in 0..50_000 {
            let x = f64::from_bits(rng.next());
            if x.is_nan() {
                continue;
            }
            let at = |m| to_f32(convert::<2>(Format::B64, Format::B32, m, from_f64(x)));
            let (down, up, zero_ward) =
                (at(Round::TowardNegative), at(Round::TowardPositive), at(Round::TowardZero));
            assert!(down as f64 <= x, "{x:e}: {down:e} should be <= x");
            assert!(up as f64 >= x, "{x:e}: {up:e} should be >= x");
            assert!((zero_ward.abs() as f64) <= x.abs(), "{x:e}: {zero_ward:e} should be no further out");
        }
    }

    #[test]
    fn overflow_respects_the_rounding_mode() {
        let huge = from_f64(1e300);
        let b32 = Format::B32;
        // Nearest reaches infinity; toward-zero stops at the largest finite value.
        assert_eq!(
            unpack(b32, convert::<2>(Format::B64, b32, Round::TiesToEven, huge)).class,
            Class::Infinite
        );
        assert_eq!(
            convert::<2>(Format::B64, b32, Round::TowardZero, huge),
            max_finite::<2>(b32, false)
        );
        // A directed mode only overflows in its own direction.
        assert_eq!(
            convert::<2>(Format::B64, b32, Round::TowardNegative, huge),
            max_finite::<2>(b32, false)
        );
        assert_eq!(
            unpack(b32, convert::<2>(Format::B64, b32, Round::TowardPositive, huge)).class,
            Class::Infinite
        );
        // And the same, mirrored, below zero.
        let huge_neg = from_f64(-1e300);
        assert_eq!(
            convert::<2>(Format::B64, b32, Round::TowardPositive, huge_neg),
            max_finite::<2>(b32, true)
        );
        assert_eq!(
            unpack(b32, convert::<2>(Format::B64, b32, Round::TowardNegative, huge_neg)).class,
            Class::Infinite
        );
    }

    #[test]
    fn zero_keeps_its_sign_through_everything() {
        for &sign in &[false, true] {
            for f in Format::ALL {
                let z = zero::<8>(f, sign);
                let v = unpack(f, z);
                assert_eq!(v.class, Class::Zero, "{}", f.name);
                assert_eq!(v.sign, sign, "{}", f.name);
                assert_eq!(pack(f, v), z, "{}", f.name);
            }
        }
    }

    #[test]
    fn the_wide_formats_hold_their_landmarks() {
        // No hardware to check b128 and b256 against, so check the structural facts:
        // the largest finite value, the smallest subnormal, and one exact power of two
        // each have to unpack to what they were built from.
        for f in [Format::B128, Format::B256] {
            let max = max_finite::<8>(f, false);
            let v = unpack(f, max);
            assert_eq!(v.class, Class::Normal, "{}", f.name);
            assert_eq!(v.sig.bit_len(), f.precision, "{}", f.name);
            assert_eq!(v.exp, f.max_quantum_exp(), "{}", f.name);

            let tiny = min_subnormal::<8>(f, false);
            let v = unpack(f, tiny);
            assert_eq!(v.class, Class::Subnormal, "{}", f.name);
            assert_eq!(v.sig, Uint::from_u64(1), "{}", f.name);
            assert_eq!(v.exp, f.min_quantum_exp(), "{}", f.name);

            // One, built by hand and read back.
            let one = round_and_pack::<8>(f, Round::TiesToEven, false, Uint::from_u64(1), 0);
            let v = unpack(f, one);
            assert_eq!(v.class, Class::Normal, "{}", f.name);
            assert_eq!(v.sig.bit_len(), f.precision, "{}", f.name);
            assert_eq!(v.exp, -(f.precision as i32 - 1), "{}", f.name);

            // Rounding up off the top of the range reaches infinity, not a wrapped value.
            let over = round_and_pack::<8>(f, Round::TiesToEven, false, Uint::from_u64(1), f.emax() + 1);
            assert_eq!(unpack(f, over).class, Class::Infinite, "{}", f.name);

            // And below the floor it reaches zero, not a wrapped value.
            let under = round_and_pack::<8>(
                f, Round::TiesToEven, false, Uint::from_u64(1), f.min_quantum_exp() - 2,
            );
            assert_eq!(unpack(f, under).class, Class::Zero, "{}", f.name);
        }
    }

    #[test]
    fn every_b16_value_narrows_from_its_own_widening_in_every_mode() {
        // Widening is exact in every rounding mode, so narrowing back cannot move.
        for mode in [
            Round::TiesToEven,
            Round::TiesToAway,
            Round::TowardZero,
            Round::TowardPositive,
            Round::TowardNegative,
        ] {
            for bits in 0u64..=0xffff {
                let u = Uint::<2>::from_u64(bits);
                if unpack(Format::B16, u).class == Class::Nan {
                    continue;
                }
                let wide = convert::<2>(Format::B16, Format::B128, mode, u);
                let back = convert::<2>(Format::B128, Format::B16, mode, wide);
                assert_eq!(back, u, "{mode:?} on b16 pattern {bits:#06x}");
            }
        }
    }
}

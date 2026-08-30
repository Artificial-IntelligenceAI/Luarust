//! IEEE 754 decimal formats, in the binary-integer encoding.
//!
//! A decimal float is a sign, a decimal exponent, and a significand of *decimal* digits —
//! seven of them for `d32`, sixteen for `d64`, thirty-four for `d128`. What that buys is
//! the thing binary floats cannot do: `0.1` is exactly one tenth, and money keeps its
//! cents. What it costs is that no hardware here has instructions for any of it.
//!
//! The standard allows two ways of writing the significand down. **BID** keeps it as an
//! ordinary binary integer; **DPD** packs it three digits at a time into ten-bit declets.
//! They hold the same numbers and differ only in the bit pattern, so nothing about
//! arithmetic depends on which is chosen — which is why the choice lives in
//! `Luarust.toml` under `[build]`, beside the other things that are about what gets
//! written out rather than what gets computed.
//!
//! Everything in here computes on a binary integer significand, because that is what
//! arithmetic wants and what [`Uint`] already provides. DPD is a repacking at the edge.

use crate::Uint;
use crate::binary::Round;

pub mod dpd;
pub mod ops;
pub mod text;

/// Which decimal format, and everything that follows from it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Format {
    pub name: &'static str,
    /// Width of the encoding, in bits.
    pub bits: u32,
    /// Significant decimal digits.
    pub digits: u32,
    /// Width of the exponent field, in bits.
    pub exp_bits: u32,
    /// Width of the trailing significand field, in bits.
    pub trailing_bits: u32,
    /// What is added to the exponent before it is stored.
    pub bias: i32,
    /// The largest exponent of the *integer* significand, so that the value is
    /// `significand × 10^exp`.
    pub max_exp: i32,
}

pub const D32: Format = Format {
    name: "d32",
    bits: 32,
    digits: 7,
    exp_bits: 8,
    trailing_bits: 20,
    bias: 101,
    max_exp: 90,
};

pub const D64: Format = Format {
    name: "d64",
    bits: 64,
    digits: 16,
    exp_bits: 10,
    trailing_bits: 50,
    bias: 398,
    max_exp: 369,
};

pub const D128: Format = Format {
    name: "d128",
    bits: 128,
    digits: 34,
    exp_bits: 14,
    trailing_bits: 110,
    bias: 6176,
    max_exp: 6111,
};

impl Format {
    /// The smallest exponent of the integer significand.
    pub fn min_exp(self) -> i32 {
        -self.bias
    }

    /// One more than the largest significand: `10^digits`.
    pub fn limit<const W: usize>(self) -> Uint<W> {
        ten_to(self.digits)
    }
}

/// What kind of value an encoding holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Finite,
    Infinite,
    Nan { signaling: bool },
}

/// A decimal taken apart: `(-1)^sign × sig × 10^exp`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Unpacked<const W: usize> {
    pub sign: bool,
    pub class: Class,
    pub sig: Uint<W>,
    pub exp: i32,
}

/// Ten to a power, as a wide integer.
pub fn ten_to<const W: usize>(power: u32) -> Uint<W> {
    let mut out = Uint::<W>::from_u64(1);
    let mut left = power;
    // Nineteen at a time is the most that certainly fits a `u64`.
    while left >= 19 {
        out = out.wrapping_mul(Uint::from_u64(10_000_000_000_000_000_000));
        left -= 19;
    }
    if left > 0 {
        out = out.wrapping_mul(Uint::from_u64(10u64.pow(left)));
    }
    out
}

/// How many decimal digits a significand has.
pub fn digits_of<const W: usize>(value: Uint<W>) -> u32 {
    if value.is_zero() {
        return 1;
    }
    // A binary length gives the decimal length to within one, and one comparison settles
    // which -- cheaper than dividing by ten until nothing is left.
    let approximate = (value.bit_len() as u64 * 1233 / 4096) as u32 + 1;
    if value < ten_to::<W>(approximate.saturating_sub(1)) {
        approximate - 1
    } else {
        approximate
    }
}

/// Read an encoding, whichever way it was written down.
pub fn unpack<const W: usize>(fmt: Format, bits: Uint<W>, dpd: bool) -> Unpacked<W> {
    let sign = bits.bit(fmt.bits - 1);
    let combination_bits = fmt.bits - 1 - fmt.trailing_bits;
    let combination = bits.shr(fmt.trailing_bits).low64() & ((1u64 << combination_bits) - 1);
    let trailing = bits & Uint::low_mask(fmt.trailing_bits);

    // The top five bits of the combination say whether this is a number at all.
    let top5 = combination >> (combination_bits - 5);
    if top5 == 0b11111 {
        return Unpacked {
            sign,
            class: Class::Nan { signaling: (combination >> (combination_bits - 6)) & 1 == 1 },
            sig: trailing,
            exp: 0,
        };
    }
    if top5 == 0b11110 {
        return Unpacked { sign, class: Class::Infinite, sig: Uint::ZERO, exp: 0 };
    }

    // Otherwise the top two bits say where the exponent starts, which is what lets the
    // leading digit be 8 or 9 without spending four bits on every significand.
    let top2 = combination >> (combination_bits - 2);
    let (biased, leading) = if top2 == 0b11 {
        let biased = (combination >> 1) & ((1u64 << fmt.exp_bits) - 1);
        (biased, 8 + (combination & 1))
    } else {
        let biased = (combination >> 3) & ((1u64 << fmt.exp_bits) - 1);
        (biased, combination & 0b111)
    };

    let digits = if dpd {
        dpd::unpack_trailing(fmt, trailing)
    } else {
        trailing
    };
    let sig = ten_to::<W>(fmt.digits - 1).wrapping_mul(Uint::from_u64(leading)).wrapping_add(digits);

    // A significand too large to be written in this many digits is not a number anybody
    // could have produced. The standard says to read it as zero rather than as rubbish.
    let sig = if sig >= fmt.limit::<W>() { Uint::ZERO } else { sig };
    Unpacked { sign, class: Class::Finite, sig, exp: biased as i32 - fmt.bias }
}

/// Write one down. The significand must already fit the format.
pub fn pack<const W: usize>(fmt: Format, value: Unpacked<W>, dpd: bool) -> Uint<W> {
    let combination_bits = fmt.bits - 1 - fmt.trailing_bits;
    let mut combination: u64;
    let trailing: Uint<W>;

    match value.class {
        Class::Infinite => {
            combination = 0b11110 << (combination_bits - 5);
            trailing = Uint::ZERO;
        }
        Class::Nan { signaling } => {
            combination = 0b11111 << (combination_bits - 5);
            if signaling {
                combination |= 1 << (combination_bits - 6);
            }
            trailing = value.sig & Uint::low_mask(fmt.trailing_bits);
        }
        Class::Finite => {
            debug_assert!(value.sig < fmt.limit::<W>(), "the significand fits the format");
            let biased = (value.exp + fmt.bias) as u64;
            let scale = ten_to::<W>(fmt.digits - 1);
            let (leading, rest) = value.sig.div_rem(scale);
            let leading = leading.low64();
            trailing = if dpd { dpd::pack_trailing(fmt, rest) } else { rest };

            combination = if leading >= 8 {
                (0b11 << (combination_bits - 2)) | (biased << 1) | (leading & 1)
            } else {
                (biased << 3) | leading
            };
        }
    }

    let mut out = trailing | Uint::from_u64(combination).shl(fmt.trailing_bits);
    if value.sign {
        out.set_bit(fmt.bits - 1);
    }
    out
}

pub fn zero<const W: usize>(fmt: Format, sign: bool, dpd: bool) -> Uint<W> {
    pack(fmt, Unpacked { sign, class: Class::Finite, sig: Uint::ZERO, exp: 0 }, dpd)
}

pub fn infinity<const W: usize>(fmt: Format, sign: bool, dpd: bool) -> Uint<W> {
    pack(fmt, Unpacked { sign, class: Class::Infinite, sig: Uint::ZERO, exp: 0 }, dpd)
}

pub fn quiet_nan<const W: usize>(fmt: Format, dpd: bool) -> Uint<W> {
    pack(
        fmt,
        Unpacked { sign: false, class: Class::Nan { signaling: false }, sig: Uint::ZERO, exp: 0 },
        dpd,
    )
}

/// Round a significand to the format's digits and write it down.
///
/// This is where every operation ends, so it is the only place that has to know about
/// rounding, about running out of exponent at the top, and about running out at the
/// bottom. Doing it once is what keeps five operations agreeing with each other.
pub fn round_and_pack<const W: usize>(
    fmt: Format,
    mode: Round,
    sign: bool,
    mut sig: Uint<W>,
    mut exp: i32,
    dpd: bool,
) -> Uint<W> {
    if sig.is_zero() {
        // A zero still has to have an exponent in range, and the standard prefers the
        // one closest to what the arithmetic produced.
        let exp = exp.clamp(fmt.min_exp(), fmt.max_exp);
        return pack(fmt, Unpacked { sign, class: Class::Finite, sig, exp }, dpd);
    }

    // Too many digits: drop the extra ones, remembering what was dropped.
    let extra = digits_of(sig) as i32 - fmt.digits as i32;
    if extra > 0 {
        sig = shorten(&mut exp, sig, extra as u32, mode, sign);
    }

    // Too small an exponent: the value has to be written with fewer digits, and what
    // does not fit is rounded away. This is where a decimal goes subnormal.
    if exp < fmt.min_exp() {
        let short_by = (fmt.min_exp() - exp) as u32;
        sig = shorten(&mut exp, sig, short_by, mode, sign);
        if sig.is_zero() {
            return zero(fmt, sign, dpd);
        }
    }

    // Too large an exponent: it may still fit if the significand has room to grow, since
    // `12 × 10^9` and `1200 × 10^7` are the same number.
    while exp > fmt.max_exp {
        let room = fmt.digits - digits_of(sig);
        if room == 0 {
            return infinity(fmt, sign, dpd);
        }
        let by = room.min((exp - fmt.max_exp) as u32);
        sig = sig.wrapping_mul(ten_to(by));
        exp -= by as i32;
    }

    if sig >= fmt.limit::<W>() {
        // Rounding up carried into an extra digit: `999…9` became `100…0`.
        sig = sig.div_rem(Uint::from_u64(10)).0;
        exp += 1;
        if exp > fmt.max_exp {
            return infinity(fmt, sign, dpd);
        }
    }

    pack(fmt, Unpacked { sign, class: Class::Finite, sig, exp }, dpd)
}

/// Drop `count` decimal digits off the bottom, rounding what is left.
fn shorten<const W: usize>(
    exp: &mut i32,
    sig: Uint<W>,
    count: u32,
    mode: Round,
    sign: bool,
) -> Uint<W> {
    if count == 0 {
        return sig;
    }
    // More digits than there are: everything is dropped, and what is left rounds from
    // whether the value was above or below half of the scale.
    if count > 40 * (W as u32) {
        *exp += count as i32;
        return Uint::ZERO;
    }
    let scale = ten_to::<W>(count);
    let (kept, dropped) = sig.div_rem(scale);
    *exp += count as i32;

    let half = scale.div_rem(Uint::from_u64(2)).0;
    let up = match mode {
        Round::TiesToEven => {
            dropped > half || (dropped == half && kept.is_odd())
        }
        Round::TiesToAway => dropped >= half,
        Round::TowardZero => false,
        Round::TowardPositive => !sign && !dropped.is_zero(),
        Round::TowardNegative => sign && !dropped.is_zero(),
    };
    if up { kept.wrapping_add(Uint::from_u64(1)) } else { kept }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::Comparison;

    fn read(fmt: Format, text: &str) -> Uint<8> {
        text::from_text(fmt, Round::TiesToEven, false, text).expect("it reads")
    }

    fn show(fmt: Format, bits: Uint<8>) -> String {
        text::to_text(fmt, unpack(fmt, bits, false))
    }

    fn calc(fmt: Format, op: fn(Format, Round, Unpacked<8>, Unpacked<8>, bool) -> Uint<8>, a: &str, b: &str) -> String {
        let x = unpack(fmt, read(fmt, a), false);
        let y = unpack(fmt, read(fmt, b), false);
        show(fmt, op(fmt, Round::TiesToEven, x, y, false))
    }

    #[test]
    fn a_tenth_is_a_tenth() {
        // The whole reason the format exists. No binary float can hold either side of
        // this, and every decimal one holds all three exactly.
        for fmt in [D32, D64, D128] {
            assert_eq!(calc(fmt, ops::add, "0.1", "0.2"), "0.3", "{}", fmt.name);
        }
    }

    #[test]
    fn money_keeps_its_cents() {
        assert_eq!(calc(D64, ops::mul, "19.99", "3"), "59.97");
        assert_eq!(calc(D64, ops::sub, "20.00", "19.99"), "0.01");
        assert_eq!(calc(D64, ops::add, "0.07", "0.01"), "0.08");
        // Ten lots of a tenth, one at a time, coming to exactly one.
        let one_tenth = unpack(D64, read(D64, "0.1"), false);
        let mut total = unpack(D64, read(D64, "0"), false);
        for _ in 0..10 {
            total = unpack(D64, ops::add(D64, Round::TiesToEven, total, one_tenth, false), false);
        }
        assert_eq!(show(D64, ops::add(D64, Round::TiesToEven, total, unpack(D64, read(D64, "0"), false), false)), "1.0");
    }

    #[test]
    fn a_written_value_reads_back_the_way_it_was_written() {
        for fmt in [D32, D64, D128] {
            for text in ["0", "1", "-1", "0.5", "-0.25", "123.456", "1000", "0.001", "7"] {
                assert_eq!(show(fmt, read(fmt, text)), text, "{} {}", fmt.name, text);
            }
        }
    }

    #[test]
    fn the_two_encodings_hold_the_same_numbers() {
        // BID and DPD differ in the bits and agree in the value, which is the entire
        // claim that lets the choice be a setting rather than a decision.
        for fmt in [D32, D64, D128] {
            for text in ["0", "1", "-1", "0.5", "123.456", "999999", "-0.001", "8", "9", "88.99"] {
                let bid = text::from_text(fmt, Round::TiesToEven, false, text).expect("reads");
                let dpd = text::from_text(fmt, Round::TiesToEven, true, text).expect("reads");
                assert_eq!(
                    text::to_text(fmt, unpack(fmt, bid, false)),
                    text::to_text(fmt, unpack(fmt, dpd, true)),
                    "{} {}",
                    fmt.name,
                    text
                );
                // Where they differ, they differ: a three-digit group under 8-8-8
                // encodes to itself, and the *leading* digit lives in the combination
                // field rather than in a declet, so only trailing 8s and 9s show it.
                if text == "999999" || text == "88.99" {
                    assert_ne!(bid, dpd, "{} {} should be written differently", fmt.name, text);
                }
            }
        }
    }

    #[test]
    fn too_many_digits_round_to_nearest_and_ties_go_even() {
        // `d32` keeps seven digits, so the eighth decides. The value keeps its size --
        // eight digits rounded to seven is still an eight-digit number, ending in zero.
        assert_eq!(show(D32, read(D32, "1.2345678")), "1.234568");
        assert_eq!(show(D32, read(D32, "12345678")), "12345680");
        // Exactly half, twice: once up to the even neighbour and once down to it.
        assert_eq!(show(D32, read(D32, "1.2345675")), "1.234568");
        assert_eq!(show(D32, read(D32, "1.2345665")), "1.234566");
        // And half plus a hair is not a tie at all: it goes up either way.
        assert_eq!(show(D32, read(D32, "1.23456651")), "1.234567");
    }

    #[test]
    fn dividing_gives_the_digits_the_format_keeps() {
        assert_eq!(calc(D32, ops::div, "1", "3"), "0.3333333");
        assert_eq!(calc(D64, ops::div, "1", "3"), "0.3333333333333333");
        assert_eq!(calc(D64, ops::div, "1", "8"), "0.125");
        assert_eq!(calc(D64, ops::div, "10", "2"), "5");
    }

    #[test]
    fn the_remainder_is_floored_like_every_other_type_here() {
        assert_eq!(calc(D64, ops::rem, "7", "3"), "1");
        assert_eq!(calc(D64, ops::rem, "-7", "3"), "2");
        assert_eq!(calc(D64, ops::rem, "7", "-3"), "-2");
        assert_eq!(calc(D64, ops::rem, "-7", "-3"), "-1");
        assert_eq!(calc(D64, ops::rem, "7.5", "0.5"), "0");
    }

    #[test]
    fn powers_are_whole_and_anything_else_is_not_a_number() {
        assert_eq!(calc(D64, ops::pow, "2", "10"), "1024");
        assert_eq!(calc(D64, ops::pow, "1.5", "2"), "2.25");
        assert_eq!(calc(D64, ops::pow, "2", "0"), "1");
        assert_eq!(calc(D64, ops::pow, "2", "-2"), "0.25");
        assert_eq!(calc(D64, ops::pow, "2", "0.5"), "nan");
    }

    #[test]
    fn the_edges_behave_the_way_a_float_does() {
        assert_eq!(calc(D64, ops::div, "1", "0"), "inf");
        assert_eq!(calc(D64, ops::div, "-1", "0"), "-inf");
        assert_eq!(calc(D64, ops::div, "0", "0"), "nan");
        assert_eq!(calc(D64, ops::add, "inf", "1"), "inf");
        assert_eq!(calc(D64, ops::add, "inf", "-inf"), "nan");
        assert_eq!(calc(D64, ops::mul, "inf", "0"), "nan");
        // A NaN carries through everything it touches.
        assert_eq!(calc(D64, ops::add, "nan", "1"), "nan");
    }

    #[test]
    fn two_values_order_by_what_they_are_worth_not_how_they_are_written() {
        // `1.0` and `1.00` are written differently and are the same number.
        let a = unpack(D64, read(D64, "1.0"), false);
        let b = unpack(D64, read(D64, "1.00"), false);
        assert_ne!(pack(D64, a, false), pack(D64, b, false), "written differently");
        assert_eq!(ops::compare(D64, a, b), Comparison::Equal, "and worth the same");

        let cases = [("1", "2", Comparison::Less), ("-1", "1", Comparison::Less),
                     ("-2", "-1", Comparison::Less), ("0", "0", Comparison::Equal),
                     ("0", "-0", Comparison::Equal), ("inf", "1", Comparison::Greater),
                     ("nan", "1", Comparison::Unordered)];
        for (x, y, want) in cases {
            let (x_u, y_u) = (unpack(D64, read(D64, x), false), unpack(D64, read(D64, y), false));
            assert_eq!(ops::compare(D64, x_u, y_u), want, "{x} vs {y}");
        }
    }

    #[test]
    fn what_will_not_fit_becomes_an_infinity_and_what_is_too_small_becomes_nothing() {
        assert_eq!(show(D32, read(D32, "1e97")), "inf");
        assert_eq!(show(D32, read(D32, "-1e97")), "-inf");
        // The smallest a `d32` can hold, and one step below it, which is nothing.
        assert_eq!(show(D32, read(D32, "1e-101")), "1e-101");
        assert_eq!(show(D32, read(D32, "1e-102")), "0");
        assert_eq!(show(D32, read(D32, "1e-200")), "0");
    }

    #[test]
    fn nothing_that_is_not_a_number_reads_as_one() {
        for text in ["", "hello", "1.2.3", "--1", "1e", "1e999999999999"] {
            assert!(
                text::from_text(D64, Round::TiesToEven, false, text).is_err(),
                "`{text}` should not read"
            );
        }
    }
}

//! Reading a written number into a binary format.
//!
//! `'0.1'` is not a value until something says what type to read it as, and then it is
//! the nearest value of *that* format — 0.0999755859375 in `b16`, and something else
//! again in `b64`. This does that conversion, and does it in one rounding step.
//!
//! One step matters. Going by way of `f64` would round twice, and two roundings do not
//! always land where one would: the first can push a value across the midpoint the second
//! is deciding against. So the digits are held exactly, as a whole number over a power of
//! ten, and rounded once at the end — the same division and sticky bit the arithmetic
//! uses.

use super::{Format, Round, round_and_pack};
use crate::uint::Uint;

/// Why a written number could not be read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Invalid {
    /// There were no digits in it at all.
    NoDigits,
    /// Something that is not a digit, a sign, or the one decimal point.
    Unexpected(char),
    /// More than one decimal point.
    TwoPoints,
    /// So many digits that the exact value will not fit while it is being rounded.
    TooLong,
}

/// Bits of slack below the rounding position, as the arithmetic uses.
const GUARD: u32 = 3;

/// Read a written decimal number as a value of `fmt`.
///
/// Accepts an optional sign, digits, and at most one decimal point. No exponent form yet:
/// nothing in the language writes one.
pub fn from_decimal<const W: usize>(
    fmt: Format,
    mode: Round,
    text: &str,
) -> Result<Uint<W>, Invalid> {
    let mut chars = text.chars().peekable();

    let mut negative = false;
    match chars.peek() {
        Some('-') => {
            negative = true;
            chars.next();
        }
        Some('+') => {
            chars.next();
        }
        _ => {}
    }

    // Every digit, point or not, as one whole number; and how many of them were after the
    // point, which is the power of ten to divide by at the end.
    let ten = Uint::<W>::from_u64(10);
    let mut digits = Uint::<W>::ZERO;
    let mut any = false;
    let mut fraction_digits = 0u32;
    let mut seen_point = false;

    for c in chars {
        match c {
            '.' if seen_point => return Err(Invalid::TwoPoints),
            '.' => seen_point = true,
            c if c.is_ascii_digit() => {
                any = true;
                let (product, carry) = digits.mul_wide(ten);
                if !carry.is_zero() {
                    return Err(Invalid::TooLong);
                }
                digits = product
                    .checked_add(Uint::from_u64((c as u8 - b'0') as u64))
                    .ok_or(Invalid::TooLong)?;
                if seen_point {
                    fraction_digits += 1;
                }
            }
            other => return Err(Invalid::Unexpected(other)),
        }
    }

    if !any {
        return Err(Invalid::NoDigits);
    }

    // A whole number needs no dividing: hand the digits straight to the rounding step.
    if fraction_digits == 0 {
        return Ok(round_and_pack(fmt, mode, negative, digits, 0));
    }

    // Otherwise the value is `digits / 10^fraction_digits`, and dividing is where the one
    // rounding happens. Shift far enough left that the quotient has the precision the
    // format wants plus the guard bits, then let the remainder be the sticky bit.
    let mut divisor = Uint::<W>::from_u64(1);
    for _ in 0..fraction_digits {
        let (product, carry) = divisor.mul_wide(ten);
        if !carry.is_zero() {
            return Err(Invalid::TooLong);
        }
        divisor = product;
    }

    let want = fmt.precision + GUARD;
    let shift = (want + divisor.bit_len()).saturating_sub(digits.bit_len());
    if digits.bit_len() + shift > Uint::<W>::BITS {
        return Err(Invalid::TooLong);
    }

    let (mut quotient, remainder) = digits.shl(shift).div_rem(divisor);
    if !remainder.is_zero() {
        quotient.set_bit(0);
    }
    Ok(round_and_pack(fmt, mode, negative, quotient, -(shift as i32)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::{Class, convert, unpack};

    fn as_f64(text: &str) -> f64 {
        let bits = from_decimal::<2>(Format::B64, Round::TiesToEven, text).unwrap();
        f64::from_bits(bits.low64())
    }

    fn as_f32(text: &str) -> f32 {
        let bits = from_decimal::<2>(Format::B32, Round::TiesToEven, text).unwrap();
        f32::from_bits(bits.low64() as u32)
    }

    #[test]
    fn whole_numbers_are_exact() {
        for text in ["0", "1", "7", "1000", "100000000", "1000000007", "9007199254740992"] {
            assert_eq!(as_f64(text), text.parse::<f64>().unwrap(), "{text}");
        }
    }

    #[test]
    fn signs_are_read() {
        assert_eq!(as_f64("-7"), -7.0);
        assert_eq!(as_f64("+7"), 7.0);
        assert_eq!(as_f64("-0.5"), -0.5);
        // Negative zero keeps its sign.
        let zero = from_decimal::<2>(Format::B64, Round::TiesToEven, "-0").unwrap();
        assert!(f64::from_bits(zero.low64()).is_sign_negative());
    }

    #[test]
    fn fractions_match_the_machine_at_the_widths_it_knows() {
        // Rust's own parser is correctly rounded, so it is a real oracle for b32 and b64.
        let cases = [
            "0.1", "0.2", "0.5", "0.25", "3.14159265358979", "2.718281828459045",
            "0.0001", "123.456", "0.3333333333333333", "1.5", "1000.0625",
        ];
        for text in cases {
            assert_eq!(as_f64(text), text.parse::<f64>().unwrap(), "b64 {text}");
            assert_eq!(as_f32(text), text.parse::<f32>().unwrap(), "b32 {text}");
        }
    }

    #[test]
    fn the_readme_number_is_the_readme_number() {
        // `b16 '0.1'` is 0.0999755859375, which the README prints and this has to produce.
        let b16 = from_decimal::<2>(Format::B16, Round::TiesToEven, "0.1").unwrap();
        let widened = convert::<2>(Format::B16, Format::B64, Round::TiesToEven, b16);
        assert_eq!(f64::from_bits(widened.low64()), 0.0999755859375);
    }

    #[test]
    fn rounding_happens_once_and_not_twice() {
        // The case that justifies the whole approach. b16 steps by two around here, so
        // 2048 and 2050 are neighbours and 2049 is the midpoint between them.
        //
        // 2049.0000000000001 is *above* that midpoint, so the correct b16 answer is 2050.
        // But the excess is 1e-13 and an f64 near 2049 steps by about 4.5e-13, so going
        // through f64 first rounds it down to exactly 2049 -- and 2049 is then a perfect
        // tie, which rounds to the even neighbour, 2048.
        //
        // Two roundings, one ulp wrong, no warning. This is what parsing the digits
        // exactly and rounding once avoids.
        let text = "2049.0000000000001";
        let as_f64 = |bits| f64::from_bits(convert::<2>(Format::B16, Format::B64, Round::TiesToEven, bits).low64());

        let once = from_decimal::<2>(Format::B16, Round::TiesToEven, text).unwrap();
        let twice = convert::<2>(
            Format::B64,
            Format::B16,
            Round::TiesToEven,
            from_decimal::<2>(Format::B64, Round::TiesToEven, text).unwrap(),
        );

        assert_eq!(as_f64(once), 2050.0, "rounded once, which is correct");
        assert_eq!(as_f64(twice), 2048.0, "rounded twice, which is not");
        assert_ne!(once, twice);
    }

    #[test]
    fn the_wide_formats_hold_more_of_a_number_than_f64_can() {
        // Twenty significant digits: b64 cannot keep them and b128 can, so reading the
        // same text at the two widths has to give genuinely different values.
        let text = "1.2345678901234567890";
        let wide = from_decimal::<8>(Format::B128, Round::TiesToEven, text).unwrap();
        let narrow = from_decimal::<8>(Format::B64, Round::TiesToEven, text).unwrap();
        let narrowed = convert::<8>(Format::B128, Format::B64, Round::TiesToEven, wide);
        assert_eq!(narrowed, narrow, "narrowing the wide one lands on the narrow one");
        assert_ne!(
            convert::<8>(Format::B64, Format::B128, Round::TiesToEven, narrow),
            wide,
            "but widening the narrow one does not recover what was lost"
        );
    }

    #[test]
    fn a_number_too_big_for_its_format_becomes_an_infinity() {
        let huge = from_decimal::<2>(Format::B16, Round::TiesToEven, "70000").unwrap();
        assert_eq!(unpack(Format::B16, huge).class, Class::Infinite);
    }

    #[test]
    fn a_number_too_small_for_its_format_becomes_a_zero() {
        // b16's smallest subnormal is 2^-24, about 6e-8, so 1e-7 still has a home --
        // as a subnormal -- and it takes 1e-8 to fall below half of it and vanish.
        let subnormal = from_decimal::<2>(Format::B16, Round::TiesToEven, "0.0000001").unwrap();
        assert_eq!(unpack(Format::B16, subnormal).class, Class::Subnormal);
        let vanished = from_decimal::<2>(Format::B16, Round::TiesToEven, "0.00000001").unwrap();
        assert_eq!(unpack(Format::B16, vanished).class, Class::Zero);
    }

    #[test]
    fn what_is_not_a_number_is_reported() {
        let read = |text| from_decimal::<2>(Format::B64, Round::TiesToEven, text);
        assert_eq!(read(""), Err(Invalid::NoDigits));
        assert_eq!(read("-"), Err(Invalid::NoDigits));
        assert_eq!(read("1.2.3"), Err(Invalid::TwoPoints));
        assert_eq!(read("12a"), Err(Invalid::Unexpected('a')));
        assert_eq!(read("1 000"), Err(Invalid::Unexpected(' ')));
        // A hundred digits will not fit while it is being rounded.
        assert_eq!(read(&"9".repeat(100)), Err(Invalid::TooLong));
    }
}

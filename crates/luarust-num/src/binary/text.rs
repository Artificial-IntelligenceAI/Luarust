//! Writing a binary float out in decimal, at whatever width it actually has.
//!
//! Going through `f64` was how this used to be done, which is exact for `b16`, `b32` and
//! `b64` and a lie for the two wider ones: a `b128` holds about thirty-four significant
//! digits and was shown seventeen of them, so it printed as though it were a `b64` and two
//! different numbers printed the same.
//!
//! Nothing arbitrary-precision is needed for it. A finite binary float is exactly
//! `sig × 2^exp`, and when `exp` is negative that is
//!
//! ```text
//!     sig            sig × 5^k
//!    -----    =     -----------  ,  k = -exp
//!     2^k              10^k
//! ```
//!
//! so **every binary float has a finite decimal expansion** -- the point simply goes `k`
//! places from the right of an integer. There is no rounding in that step and no
//! searching; it is a multiplication.
//!
//! What is shown is that expansion, whole. Not the shortest run of digits that reads back
//! as the same number, which is what most languages print and what `f64` printing gave
//! here: `b64 |0.1|` is not one tenth and never was, and showing `0.1` says it is. It is
//!
//! ```text
//!     0.1000000000000000055511151231257827021181583404541015625
//! ```
//!
//! and that is what a program that would rather not guess should say. The same reasoning
//! put `er` in the language and made the decimal formats print their digits out: a value
//! is shown as what it is, not as the text somebody typed to make it.

use super::{Class, Format, Round, literal, unpack};
use crate::big::Big;
use crate::uint::Uint;

/// `5^power`, by squaring.
fn five_to(power: u32) -> Big {
    let mut out = Big::one();
    let mut base = Big::from_u64(5);
    let mut left = power;
    while left > 0 {
        if left & 1 == 1 {
            out = out.mul(&base);
        }
        base = base.mul(&base);
        left >>= 1;
    }
    out
}

/// A `Uint` as a `Big`, which is the same number in the shape arithmetic wants.
fn widen<const W: usize>(value: Uint<W>) -> Big {
    Big::from_parts(false, value.limbs().to_vec())
}

/// The exact digits of a finite float, and where the point goes.
///
/// Returns the digits with no point in them and how many of them are after it. Exact:
/// nothing here rounds.
fn exact<const W: usize>(sig: Uint<W>, exp: i32) -> (String, usize) {
    let value = widen(sig);
    if exp >= 0 {
        (value.shl(exp as u32).to_string(), 0)
    } else {
        let k = exp.unsigned_abs();
        (value.mul(&five_to(k)).to_string(), k as usize)
    }
}

/// Digits and a point position, written the way a person reads a number.
///
/// `digits` is an integer and `after_point` says how many of its digits are to the right
/// of the point, so the value is `digits × 10^-after_point`.
fn place(negative: bool, digits: &str, after_point: i32) -> String {
    let sign = if negative { "-" } else { "" };

    // Trailing zeros in the *fraction* say nothing, and dropping one divides the digits by
    // ten -- so the point has to come in by one for each, or the number changes. That step
    // being missed is what turned `0.0999755859375` into `0.00999755859375`.
    let mut digits = digits;
    let mut after_point = after_point;
    while after_point > 0 && digits.len() > 1 && digits.ends_with('0') {
        digits = &digits[..digits.len() - 1];
        after_point -= 1;
    }
    let digits = digits.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };

    if after_point <= 0 {
        // An integer, with however many zeros the exponent asks for after it.
        return format!("{sign}{digits}{}", "0".repeat(after_point.unsigned_abs() as usize));
    }
    let after_point = after_point as usize;
    if after_point >= digits.len() {
        return format!("{sign}0.{}{digits}", "0".repeat(after_point - digits.len()));
    }
    let (whole, fraction) = digits.split_at(digits.len() - after_point);
    format!("{sign}{whole}.{fraction}")
}

/// Round a run of digits to `keep` of them, carrying where it must.
///
/// Returns the kept digits and how far the point moved, which is one place when a carry
/// runs off the end -- `999` to two digits is `10`, and the value is ten times the digits.
fn shorten(digits: &str, keep: usize) -> (String, i32) {
    if keep >= digits.len() {
        return (digits.to_string(), 0);
    }
    let bytes = digits.as_bytes();
    let mut kept: Vec<u8> = bytes[..keep].to_vec();

    if bytes[keep] >= b'5' {
        let mut at = keep;
        loop {
            if at == 0 {
                kept.insert(0, b'1');
                kept.pop();
                return (String::from_utf8(kept).expect("digits"), 1);
            }
            at -= 1;
            if kept[at] == b'9' {
                kept[at] = b'0';
            } else {
                kept[at] += 1;
                break;
            }
        }
    }
    (String::from_utf8(kept).expect("digits"), 0)
}

/// A finite float as the shortest decimal that reads back as itself.
///
/// The other way of writing one: the fewest digits that name this number and no other at
/// this format. `b64 |0.1|` is `0.1` again, and a `b128` still shows its own thirty-four
/// rather than a `b64`'s seventeen -- what it does not do is show that `0.1` is not one
/// tenth, which is the whole of the difference between the two.
///
/// Finding it is a loop: round the exact digits to one significant digit, read it back,
/// and if it is not the same number try two. Reading back uses the same correctly-rounded
/// parser a program's own literals go through, so "reads back" means exactly that.
pub fn to_shortest<const W: usize>(fmt: Format, bits: Uint<W>) -> Option<String> {
    let taken = unpack(fmt, bits);
    match taken.class {
        Class::Nan | Class::Infinite => return None,
        Class::Zero => return Some(if taken.sign { "-0".to_string() } else { "0".to_string() }),
        _ => {}
    }

    let (digits, after_point) = exact(taken.sig, taken.exp);
    let trimmed = digits.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    let lost = digits.len() - trimmed.len();

    // The exact form always reads back, so this ends; it stops at the first width that
    // does, which is the shortest.
    for keep in 1..=trimmed.len() {
        let (shortened, carried) = shorten(trimmed, keep);
        let dropped = trimmed.len() - keep;
        let point = after_point as i32 - lost as i32 - dropped as i32 + carried;
        let written = place(taken.sign, &shortened, point);
        if literal::from_decimal::<W>(fmt, Round::TiesToEven, &written) == Ok(bits) {
            return Some(written);
        }
    }
    Some(place(taken.sign, &digits, after_point as i32))
}

/// A finite float, written out exactly.
///
/// `None` for anything that is not finite; those are named rather than written.
pub fn to_text<const W: usize>(fmt: Format, bits: Uint<W>) -> Option<String> {
    let taken = unpack(fmt, bits);
    match taken.class {
        Class::Nan | Class::Infinite => return None,
        Class::Zero => return Some(if taken.sign { "-0".to_string() } else { "0".to_string() }),
        _ => {}
    }

    let (digits, after_point) = exact(taken.sig, taken.exp);
    Some(place(taken.sign, &digits, after_point as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::{Format, Round, literal};

    fn round<const W: usize>(fmt: Format, text: &str) -> String {
        let bits = literal::from_decimal::<W>(fmt, Round::TiesToEven, text).expect("a literal");
        to_text(fmt, bits).expect("a finite number")
    }

    /// The point of the whole module: what is printed is what is held.
    #[test]
    fn a_tenth_is_not_a_tenth() {
        // None of these is one tenth, and none of them says it is.
        assert_eq!(round::<8>(Format::B16, "0.1"), "0.0999755859375");
        assert_eq!(round::<8>(Format::B32, "0.1"), "0.100000001490116119384765625");
        assert_eq!(
            round::<8>(Format::B64, "0.1"),
            "0.1000000000000000055511151231257827021181583404541015625"
        );
        // The decimal formats do not have the problem in the first place.
        assert_eq!(round::<8>(Format::B64, "0.5"), "0.5", "a half is exact in binary");
        assert_eq!(round::<8>(Format::B64, "0.25"), "0.25");
    }

    #[test]
    fn what_is_exact_stays_short() {
        for text in ["0", "-0", "1", "-1", "0.5", "100", "0.125", "-7.25", "1024"] {
            assert_eq!(round::<8>(Format::B64, text), text, "{text} is exact in binary");
        }
    }

    /// A wide float shows its own width rather than a `b64`'s.
    #[test]
    fn a_wide_float_shows_its_own_width() {
        let third = |fmt: Format| {
            let one = literal::from_decimal::<8>(fmt, Round::TiesToEven, "1").expect("1");
            let three = literal::from_decimal::<8>(fmt, Round::TiesToEven, "3").expect("3");
            to_text(fmt, super::super::arith::div(fmt, Round::TiesToEven, one, three))
                .expect("finite")
        };
        let (a, b, c) = (third(Format::B64), third(Format::B128), third(Format::B256));
        assert!(a.len() < b.len(), "a b128 third is longer than a b64 one");
        assert!(b.len() < c.len(), "and a b256 third longer still");
        assert_ne!(a, b);
        assert_ne!(b, c);
    }

    /// Exact means it reads back. At every format, with nothing left over.
    #[test]
    fn every_written_value_reads_back_as_itself() {
        for fmt in [Format::B16, Format::B32, Format::B64, Format::B128, Format::B256] {
            for text in ["0.1", "1", "3", "0.333333333333333333333333333", "12345.6789", "-7.25"] {
                let bits = literal::from_decimal::<8>(fmt, Round::TiesToEven, text).expect("read");
                let written = to_text(fmt, bits).expect("finite");
                let back = literal::from_decimal::<8>(fmt, Round::TiesToEven, &written);
                assert_eq!(back, Ok(bits), "{} wrote {text} as {written}", fmt.name);
            }
        }
    }

    /// Both ways of writing a float name the same number.
    ///
    /// Which is the point of offering both: they disagree about how much to say, never
    /// about what is true. Whatever either writes reads back as the number it came from.
    #[test]
    fn exact_and_shortest_are_the_same_number() {
        for fmt in [Format::B16, Format::B32, Format::B64, Format::B128] {
            for text in ["0.1", "1", "0.25", "3", "-7.25", "12345.6789"] {
                let bits = literal::from_decimal::<8>(fmt, Round::TiesToEven, text).expect("read");
                let long = to_text(fmt, bits).expect("finite");
                let short = to_shortest(fmt, bits).expect("finite");
                assert!(short.len() <= long.len(), "{short} is not shorter than {long}");
                for written in [&long, &short] {
                    assert_eq!(
                        literal::from_decimal::<8>(fmt, Round::TiesToEven, written),
                        Ok(bits),
                        "{} wrote {text} as {written}",
                        fmt.name
                    );
                }
            }
        }
        // And the difference they exist for.
        let tenth = literal::from_decimal::<8>(Format::B64, Round::TiesToEven, "0.1").expect("r");
        assert_eq!(to_shortest(Format::B64, tenth).unwrap(), "0.1");
        assert!(to_text(Format::B64, tenth).unwrap().len() > 50, "the exact one is long");
    }

    /// A `b256` prints about two hundred and forty digits, and reads every one of them.
    ///
    /// It did not, for a while: the parser accumulated a literal's digits into the width
    /// the answer came back in, which stopped near a hundred and fifty of them, so what a
    /// `b256` printed could not be pasted back into a program.
    #[test]
    fn a_b256_reads_back_everything_it_prints() {
        let bits = literal::from_decimal::<8>(Format::B256, Round::TiesToEven, "0.1").expect("read");
        let written = to_text(Format::B256, bits).expect("finite");
        let digits = written.chars().filter(char::is_ascii_digit).count();
        assert!(digits > 200, "a b256 tenth is about 240 digits, not {digits}");
        assert_eq!(
            literal::from_decimal::<8>(Format::B256, Round::TiesToEven, &written),
            Ok(bits),
            "what it printed should read back as the number it printed"
        );
    }
}

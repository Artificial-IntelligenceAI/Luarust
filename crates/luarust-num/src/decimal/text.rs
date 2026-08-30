//! Decimals as they are written and read.
//!
//! A decimal float can be written out exactly, always, which is the whole reason for
//! having one: the significand *is* decimal digits, so printing it is arranging digits
//! rather than searching for the shortest decimal that reads back the same. A binary
//! float has to do that search; this one never does.

use super::{Class, Format, Unpacked, digits_of, round_and_pack, ten_to};
use crate::Uint;
use crate::binary::Round;

/// What a written number could not be read as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bad {
    /// Nothing that looks like a number at all.
    NotANumber,
    /// More digits than any exponent could place.
    OutOfRange,
}

/// Read a written decimal into a format, rounding if it has more digits than fit.
pub fn from_text(fmt: Format, mode: Round, dpd: bool, text: &str) -> Result<Uint<8>, Bad> {
    let text = text.trim();
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    if rest.is_empty() {
        return Err(Bad::NotANumber);
    }

    // The words a float can be, which a decimal can be too.
    match rest.to_ascii_lowercase().as_str() {
        "inf" | "infinity" => return Ok(super::infinity(fmt, negative, dpd)),
        "nan" => return Ok(super::quiet_nan(fmt, dpd)),
        _ => {}
    }

    // An exponent, if there is one.
    let (mantissa, written_exp) = match rest.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => {
            let exponent: i32 = exponent.parse().map_err(|_| Bad::NotANumber)?;
            (mantissa, exponent)
        }
        None => (rest, 0),
    };

    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (mantissa, ""),
    };
    if whole.is_empty() && fraction.is_empty() {
        return Err(Bad::NotANumber);
    }
    if !whole.bytes().chain(fraction.bytes()).all(|b| b.is_ascii_digit()) {
        return Err(Bad::NotANumber);
    }

    // The digits as one integer, with the exponent carrying where the point was. More
    // digits than the working width could hold cannot change the answer's leading ones,
    // so they are dropped -- but the last kept digit is nudged, so a tie still knows
    // there was something below it.
    let all: String = format!("{whole}{fraction}");
    let trimmed = all.trim_start_matches('0');
    let keep = 2 * fmt.digits as usize + 4;
    let (digits, dropped) = if trimmed.len() > keep {
        (&trimmed[..keep], trimmed.len() - keep)
    } else {
        (trimmed, 0)
    };

    let mut sig = Uint::<8>::ZERO;
    for chunk in digits.as_bytes().chunks(19) {
        let text = std::str::from_utf8(chunk).expect("digits are ASCII");
        let value: u64 = text.parse().map_err(|_| Bad::NotANumber)?;
        sig = sig.wrapping_mul(ten_to(chunk.len() as u32)).wrapping_add(Uint::from_u64(value));
    }
    if dropped > 0 && !trimmed[keep..].bytes().all(|b| b == b'0') {
        sig = sig | Uint::from_u64(1);
    }

    let exp = written_exp
        .checked_sub(fraction.len() as i32)
        .and_then(|e| e.checked_add(dropped as i32))
        .ok_or(Bad::OutOfRange)?;

    Ok(round_and_pack(fmt, mode, negative, sig, exp, dpd))
}

/// Write one out, exactly.
pub fn to_text(fmt: Format, value: Unpacked<8>) -> String {
    match value.class {
        Class::Nan { .. } => return "nan".to_string(),
        Class::Infinite => return if value.sign { "-inf".into() } else { "inf".into() },
        Class::Finite => {}
    }

    let sign = if value.sign { "-" } else { "" };
    let digits = digits_string(value.sig);
    if value.sig.is_zero() {
        return format!("{sign}0");
    }

    let count = digits.len() as i32;
    // Where the point falls, counting from the left of the digits.
    let point = count + value.exp;

    // An exponent far from the digits would need more zeros than anyone wants to read,
    // so those get written the way a calculator writes them.
    if point > fmt.digits as i32 + 6 || point < -5 {
        let head = &digits[..1];
        let tail = &digits[1..];
        let exponent = point - 1;
        return if tail.is_empty() {
            format!("{sign}{head}e{exponent:+}")
        } else {
            format!("{sign}{head}.{tail}e{exponent:+}")
        };
    }

    if value.exp >= 0 {
        return format!("{sign}{digits}{}", "0".repeat(value.exp as usize));
    }
    if point <= 0 {
        return format!("{sign}0.{}{digits}", "0".repeat((-point) as usize));
    }
    let (whole, fraction) = digits.split_at(point as usize);
    format!("{sign}{whole}.{fraction}")
}

fn digits_string(sig: Uint<8>) -> String {
    if sig.is_zero() {
        return "0".to_string();
    }
    let mut out = String::new();
    let mut left = sig;
    let chunk = Uint::<8>::from_u64(10_000_000_000_000_000_000);
    while !left.is_zero() {
        let (next, remainder) = left.div_rem(chunk);
        if next.is_zero() {
            out.push_str(&remainder.low64().to_string().chars().rev().collect::<String>());
        } else {
            out.push_str(&format!("{:019}", remainder.low64()).chars().rev().collect::<String>());
        }
        left = next;
    }
    out.chars().rev().collect()
}

/// The number of digits, for anyone who wants to know without the string.
pub fn digit_count(sig: Uint<8>) -> u32 {
    digits_of(sig)
}

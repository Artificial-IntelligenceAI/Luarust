//! Whole numbers with no width.
//!
//! Everything else in this crate is a fixed number of bits, because a `b256` is exactly
//! 256 bits and an `i64` is exactly 64. An exact rational is not: adding two of them
//! multiplies their denominators, and nothing bounds how large that gets. So this one
//! grows.
//!
//! It is a sign and a magnitude in little-endian 64-bit limbs, with no leading zero limb,
//! and zero is always positive. Keeping those two invariants is what makes comparison and
//! equality cheap and unambiguous — there is one representation of every number.

use std::cmp::Ordering;

/// A whole number of any size.
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Big {
    /// Always `false` for zero, so a number has one representation and not two.
    negative: bool,
    /// Little-endian, and never ending in a zero limb.
    limbs: Vec<u64>,
}

impl Big {
    pub fn zero() -> Self {
        Big { negative: false, limbs: Vec::new() }
    }

    pub fn one() -> Self {
        Big { negative: false, limbs: vec![1] }
    }

    pub fn from_u64(value: u64) -> Self {
        Big { negative: false, limbs: if value == 0 { Vec::new() } else { vec![value] } }
    }

    pub fn from_i64(value: i64) -> Self {
        Big { negative: value < 0, limbs: match value.unsigned_abs() {
            0 => Vec::new(),
            n => vec![n],
        } }
    }

    /// The limbs of the magnitude, little-endian.
    pub fn limbs(&self) -> &[u64] {
        &self.limbs
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    pub fn is_one(&self) -> bool {
        !self.negative && self.limbs == [1]
    }

    /// Build one from a sign and limbs that may have leading zeros.
    pub fn from_parts(negative: bool, mut limbs: Vec<u64>) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        let negative = negative && !limbs.is_empty();
        Big { negative, limbs }
    }

    pub fn negated(&self) -> Self {
        Big::from_parts(!self.negative, self.limbs.clone())
    }

    pub fn magnitude(&self) -> Self {
        Big::from_parts(false, self.limbs.clone())
    }

    /// The value as a `u64`, when it fits.
    pub fn to_u64(&self) -> Option<u64> {
        match self.limbs.as_slice() {
            [] => Some(0),
            [only] if !self.negative => Some(*only),
            _ => None,
        }
    }

    fn cmp_magnitude(a: &[u64], b: &[u64]) -> Ordering {
        if a.len() != b.len() {
            return a.len().cmp(&b.len());
        }
        for (x, y) in a.iter().zip(b).rev() {
            if x != y {
                return x.cmp(y);
            }
        }
        Ordering::Equal
    }

    fn add_magnitudes(a: &[u64], b: &[u64]) -> Vec<u64> {
        let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
        let mut carry = 0u64;
        for i in 0..a.len().max(b.len()) {
            let x = a.get(i).copied().unwrap_or(0) as u128;
            let y = b.get(i).copied().unwrap_or(0) as u128;
            let sum = x + y + carry as u128;
            out.push(sum as u64);
            carry = (sum >> 64) as u64;
        }
        if carry != 0 {
            out.push(carry);
        }
        out
    }

    /// `a - b`, where `a` is known to be at least `b`.
    fn sub_magnitudes(a: &[u64], b: &[u64]) -> Vec<u64> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0i128;
        for (i, limb) in a.iter().enumerate() {
            let x = *limb as i128;
            let y = b.get(i).copied().unwrap_or(0) as i128;
            let mut diff = x - y - borrow;
            borrow = if diff < 0 { 1 } else { 0 };
            if diff < 0 {
                diff += 1i128 << 64;
            }
            out.push(diff as u64);
        }
        while out.last() == Some(&0) {
            out.pop();
        }
        out
    }

    pub fn add(&self, other: &Big) -> Big {
        if self.negative == other.negative {
            return Big::from_parts(self.negative, Self::add_magnitudes(&self.limbs, &other.limbs));
        }
        match Self::cmp_magnitude(&self.limbs, &other.limbs) {
            Ordering::Equal => Big::zero(),
            Ordering::Greater => {
                Big::from_parts(self.negative, Self::sub_magnitudes(&self.limbs, &other.limbs))
            }
            Ordering::Less => {
                Big::from_parts(other.negative, Self::sub_magnitudes(&other.limbs, &self.limbs))
            }
        }
    }

    pub fn sub(&self, other: &Big) -> Big {
        self.add(&other.negated())
    }

    pub fn mul(&self, other: &Big) -> Big {
        if self.is_zero() || other.is_zero() {
            return Big::zero();
        }
        let mut limbs = vec![0u64; self.limbs.len() + other.limbs.len()];
        for (i, x) in self.limbs.iter().enumerate() {
            let mut carry = 0u128;
            for (j, y) in other.limbs.iter().enumerate() {
                let at = i + j;
                let sum = limbs[at] as u128 + (*x as u128) * (*y as u128) + carry;
                limbs[at] = sum as u64;
                carry = sum >> 64;
            }
            let mut at = i + other.limbs.len();
            while carry != 0 {
                let sum = limbs[at] as u128 + carry;
                limbs[at] = sum as u64;
                carry = sum >> 64;
                at += 1;
            }
        }
        Big::from_parts(self.negative != other.negative, limbs)
    }

    fn shl_one(limbs: &mut Vec<u64>) {
        let mut carry = 0u64;
        for limb in limbs.iter_mut() {
            let next = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = next;
        }
        if carry != 0 {
            limbs.push(carry);
        }
    }

    /// Truncated division: the quotient rounds towards zero and the remainder takes the
    /// sign of the left side, which is what every integer division here already does.
    ///
    /// Shift-and-subtract, one bit at a time. Slower than the schoolbook algorithm on
    /// long numbers and very much easier to be sure of, which for the only division in
    /// the crate that has no fixed width is the right way round.
    pub fn div_rem(&self, other: &Big) -> Option<(Big, Big)> {
        if other.is_zero() {
            return None;
        }
        if Self::cmp_magnitude(&self.limbs, &other.limbs) == Ordering::Less {
            return Some((Big::zero(), self.clone()));
        }

        let divisor = &other.limbs;
        let bits = self.limbs.len() * 64;
        let mut quotient = vec![0u64; self.limbs.len()];
        let mut remainder: Vec<u64> = Vec::new();

        for bit in (0..bits).rev() {
            Self::shl_one(&mut remainder);
            if (self.limbs[bit / 64] >> (bit % 64)) & 1 == 1 {
                if remainder.is_empty() {
                    remainder.push(1);
                } else {
                    remainder[0] |= 1;
                }
            }
            if Self::cmp_magnitude(&remainder, divisor) != Ordering::Less {
                remainder = Self::sub_magnitudes(&remainder, divisor);
                quotient[bit / 64] |= 1 << (bit % 64);
            }
        }

        Some((
            Big::from_parts(self.negative != other.negative, quotient),
            Big::from_parts(self.negative, remainder),
        ))
    }

    /// The greatest common divisor of two magnitudes, by halving and subtracting rather
    /// than dividing -- which for numbers this shape is both faster and simpler.
    pub fn gcd(a: &Big, b: &Big) -> Big {
        let mut x = a.magnitude();
        let mut y = b.magnitude();
        if x.is_zero() {
            return y;
        }
        if y.is_zero() {
            return x;
        }

        let shared = x.trailing_zeros().min(y.trailing_zeros());
        x = x.shr(x.trailing_zeros());
        loop {
            y = y.shr(y.trailing_zeros());
            if Self::cmp_magnitude(&x.limbs, &y.limbs) == Ordering::Greater {
                std::mem::swap(&mut x, &mut y);
            }
            y = Big::from_parts(false, Self::sub_magnitudes(&y.limbs, &x.limbs));
            if y.is_zero() {
                return x.shl(shared);
            }
        }
    }

    fn trailing_zeros(&self) -> u32 {
        for (i, limb) in self.limbs.iter().enumerate() {
            if *limb != 0 {
                return (i as u32) * 64 + limb.trailing_zeros();
            }
        }
        0
    }

    pub fn shl(&self, n: u32) -> Big {
        if self.is_zero() || n == 0 {
            return self.clone();
        }
        let (whole, part) = ((n / 64) as usize, n % 64);
        let mut limbs = vec![0u64; whole];
        let mut carry = 0u64;
        for limb in &self.limbs {
            if part == 0 {
                limbs.push(*limb);
            } else {
                limbs.push((*limb << part) | carry);
                carry = *limb >> (64 - part);
            }
        }
        if carry != 0 {
            limbs.push(carry);
        }
        Big::from_parts(self.negative, limbs)
    }

    pub fn shr(&self, n: u32) -> Big {
        if self.is_zero() || n == 0 {
            return self.clone();
        }
        let (whole, part) = ((n / 64) as usize, n % 64);
        if whole >= self.limbs.len() {
            return Big::zero();
        }
        let mut limbs: Vec<u64> = self.limbs[whole..].to_vec();
        if part != 0 {
            for i in 0..limbs.len() {
                let high = limbs.get(i + 1).copied().unwrap_or(0);
                limbs[i] = (limbs[i] >> part) | (high << (64 - part));
            }
        }
        Big::from_parts(self.negative, limbs)
    }

    /// Multiply by a small number and add another, which is how a written number is read.
    fn mul_add_small(&mut self, factor: u64, addend: u64) {
        let mut carry = addend as u128;
        for limb in self.limbs.iter_mut() {
            let sum = (*limb as u128) * (factor as u128) + carry;
            *limb = sum as u64;
            carry = sum >> 64;
        }
        while carry != 0 {
            self.limbs.push(carry as u64);
            carry >>= 64;
        }
    }

    /// Divide by a small number, giving the remainder. Used only for writing one out.
    fn div_small(&self, divisor: u64) -> (Big, u64) {
        let mut limbs = self.limbs.clone();
        let mut remainder = 0u128;
        for limb in limbs.iter_mut().rev() {
            let value = (remainder << 64) | (*limb as u128);
            *limb = (value / divisor as u128) as u64;
            remainder = value % divisor as u128;
        }
        (Big::from_parts(self.negative, limbs), remainder as u64)
    }

    /// Ten to the power of a small number, for reading a written decimal.
    pub fn ten_to(power: u32) -> Big {
        let mut out = Big::one();
        let mut left = power;
        while left >= 19 {
            out.mul_add_small(10_000_000_000_000_000_000, 0);
            left -= 19;
        }
        if left > 0 {
            out.mul_add_small(10u64.pow(left), 0);
        }
        out
    }

    /// Read a run of decimal digits. Nothing else is accepted, sign included.
    pub fn from_digits(digits: &str) -> Option<Big> {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let mut out = Big::zero();
        // Nineteen digits at a time, since that is the most that certainly fits a `u64`.
        for chunk in digits.as_bytes().chunks(19) {
            let text = std::str::from_utf8(chunk).expect("digits are ASCII");
            let value: u64 = text.parse().ok()?;
            out.mul_add_small(10u64.pow(chunk.len() as u32), value);
        }
        Some(out)
    }
}

impl std::cmp::Ord for Big {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => Big::cmp_magnitude(&self.limbs, &other.limbs),
            (true, true) => Big::cmp_magnitude(&other.limbs, &self.limbs),
        }
    }
}

impl std::cmp::PartialOrd for Big {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Big {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_zero() {
            return f.write_str("0");
        }
        let mut digits = String::new();
        let mut left = self.magnitude();
        while !left.is_zero() {
            let (next, remainder) = left.div_small(10_000_000_000_000_000_000);
            if next.is_zero() {
                digits.push_str(&remainder.to_string().chars().rev().collect::<String>());
            } else {
                digits.push_str(&format!("{remainder:019}").chars().rev().collect::<String>());
            }
            left = next;
        }
        if self.negative {
            f.write_str("-")?;
        }
        f.write_str(&digits.chars().rev().collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big(text: &str) -> Big {
        match text.strip_prefix('-') {
            Some(rest) => Big::from_digits(rest).expect("digits").negated(),
            None => Big::from_digits(text).expect("digits"),
        }
    }

    #[test]
    fn a_number_reads_back_the_way_it_was_written() {
        for text in [
            "0",
            "1",
            "9",
            "10",
            "18446744073709551615",
            "18446744073709551616",
            "340282366920938463463374607431768211456",
            "99999999999999999999999999999999999999999999999999",
        ] {
            assert_eq!(big(text).to_string(), text, "{text}");
            assert_eq!(big(&format!("-{text}")).to_string(), if text == "0" { "0".into() } else { format!("-{text}") });
        }
    }

    #[test]
    fn zero_has_one_shape_and_it_is_not_negative() {
        assert_eq!(Big::zero(), big("0").negated());
        assert!(!big("0").negated().is_negative());
        assert_eq!(big("5").sub(&big("5")), Big::zero());
    }

    #[test]
    fn arithmetic_agrees_with_what_a_u128_would_have_said() {
        let cases: [(i128, i128); 10] = [
            (0, 0),
            (1, 1),
            (7, 3),
            (-7, 3),
            (7, -3),
            (-7, -3),
            (1 << 70, 3),
            (-(1 << 70), 12345),
            (123456789012345678, 987654321098765),
            (i64::MAX as i128, i64::MIN as i128),
        ];
        for (a, b) in cases {
            let (x, y) = (big(&a.to_string()), big(&b.to_string()));
            assert_eq!(x.add(&y).to_string(), (a + b).to_string(), "{a} + {b}");
            assert_eq!(x.sub(&y).to_string(), (a - b).to_string(), "{a} - {b}");
            assert_eq!(x.mul(&y).to_string(), (a * b).to_string(), "{a} * {b}");
            if b != 0 {
                let (q, r) = x.div_rem(&y).expect("not by zero");
                assert_eq!(q.to_string(), (a / b).to_string(), "{a} / {b}");
                assert_eq!(r.to_string(), (a % b).to_string(), "{a} rem {b}");
            }
        }
    }

    #[test]
    fn dividing_by_nothing_is_nothing_rather_than_a_panic() {
        assert!(big("5").div_rem(&Big::zero()).is_none());
    }

    #[test]
    fn a_common_divisor_is_the_greatest_one() {
        assert_eq!(Big::gcd(&big("12"), &big("18")).to_string(), "6");
        assert_eq!(Big::gcd(&big("17"), &big("5")).to_string(), "1");
        assert_eq!(Big::gcd(&big("0"), &big("7")).to_string(), "7");
        assert_eq!(Big::gcd(&big("-12"), &big("18")).to_string(), "6");
        // One that no `u64` could hold, so the answer cannot have come from a shortcut.
        let a = big("123456789012345678901234567890");
        let b = big("9876543210987654321098765432");
        assert_eq!(Big::gcd(&a, &b).to_string(), "2");
    }

    #[test]
    fn shifting_is_multiplying_and_dividing_by_two() {
        let n = big("123456789012345678901234567890");
        assert_eq!(n.shl(1).to_string(), n.add(&n).to_string());
        assert_eq!(n.shl(64).shr(64).to_string(), n.to_string());
        assert_eq!(big("1").shl(100).to_string(), "1267650600228229401496703205376");
    }

    #[test]
    fn ten_to_a_power_is_a_one_and_that_many_noughts() {
        for power in [0u32, 1, 5, 19, 20, 40] {
            let text = Big::ten_to(power).to_string();
            assert_eq!(text.len(), power as usize + 1, "10^{power}");
            assert!(text.starts_with('1') && text[1..].bytes().all(|b| b == b'0'));
        }
    }
}

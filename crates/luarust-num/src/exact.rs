//! `er` — an exact rational.
//!
//! A numerator over a denominator, both unbounded, always in lowest terms with the sign
//! on the numerator and the denominator strictly positive. There is one representation of
//! every value, so two of these are equal exactly when they are the same number.
//!
//! Nothing here rounds and nothing here overflows. That is the whole point of the type:
//! `0.1 + 0.2` is `3/10`, not `0.30000000000000004`, and a third is a third rather than
//! the nearest thing to it. What it costs is that arithmetic allocates, and that
//! denominators grow when you add fractions with nothing in common.

use crate::big::Big;
use std::cmp::Ordering;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Exact {
    /// Carries the sign.
    numerator: Big,
    /// Never zero, never negative.
    denominator: Big,
}

/// What went wrong, when something did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trouble {
    DivideByZero,
    /// A power that is not a whole number, whose answer is generally not rational.
    FractionalPower,
    /// A power large enough that working it out would be the whole afternoon.
    PowerTooLarge,
}

impl Exact {
    /// The largest exponent `**` will work out. Ten thousand of anything is already a
    /// number with thousands of digits; past that the answer is a denial of service
    /// rather than a result.
    pub const POWER_LIMIT: u64 = 10_000;

    pub fn zero() -> Self {
        Exact { numerator: Big::zero(), denominator: Big::one() }
    }

    pub fn one() -> Self {
        Exact { numerator: Big::one(), denominator: Big::one() }
    }

    pub fn is_zero(&self) -> bool {
        self.numerator.is_zero()
    }

    pub fn numerator(&self) -> &Big {
        &self.numerator
    }

    pub fn denominator(&self) -> &Big {
        &self.denominator
    }

    /// A ratio, put in lowest terms with the sign on top. `None` if the bottom is zero.
    pub fn ratio(numerator: Big, denominator: Big) -> Option<Self> {
        if denominator.is_zero() {
            return None;
        }
        let flip = denominator.is_negative();
        let numerator = if flip { numerator.negated() } else { numerator };
        let denominator = denominator.magnitude();

        if numerator.is_zero() {
            return Some(Exact::zero());
        }
        let common = Big::gcd(&numerator, &denominator);
        if common.is_one() {
            return Some(Exact { numerator, denominator });
        }
        let (numerator, _) = numerator.div_rem(&common).expect("a common divisor is not zero");
        let (denominator, _) = denominator.div_rem(&common).expect("a common divisor is not zero");
        Some(Exact { numerator, denominator })
    }

    pub fn whole(value: Big) -> Self {
        Exact { numerator: value, denominator: Big::one() }
    }

    pub fn negated(&self) -> Self {
        Exact { numerator: self.numerator.negated(), denominator: self.denominator.clone() }
    }

    pub fn add(&self, other: &Exact) -> Exact {
        let numerator = self
            .numerator
            .mul(&other.denominator)
            .add(&other.numerator.mul(&self.denominator));
        Exact::ratio(numerator, self.denominator.mul(&other.denominator))
            .expect("neither denominator is zero")
    }

    pub fn sub(&self, other: &Exact) -> Exact {
        self.add(&other.negated())
    }

    pub fn mul(&self, other: &Exact) -> Exact {
        Exact::ratio(
            self.numerator.mul(&other.numerator),
            self.denominator.mul(&other.denominator),
        )
        .expect("neither denominator is zero")
    }

    pub fn div(&self, other: &Exact) -> Result<Exact, Trouble> {
        if other.is_zero() {
            return Err(Trouble::DivideByZero);
        }
        Exact::ratio(
            self.numerator.mul(&other.denominator),
            self.denominator.mul(&other.numerator),
        )
        .ok_or(Trouble::DivideByZero)
    }

    /// The largest whole number at or below this one.
    pub fn floor(&self) -> Big {
        let (quotient, remainder) =
            self.numerator.div_rem(&self.denominator).expect("the bottom is never zero");
        // Truncation rounds towards zero, and the floor rounds down, so a negative
        // number with anything left over is one lower.
        if self.numerator.is_negative() && !remainder.is_zero() {
            quotient.sub(&Big::one())
        } else {
            quotient
        }
    }

    /// Floored remainder: what is left after taking away as many whole `other`s as fit,
    /// counting downwards. The answer takes the sign of the divisor, as it does for every
    /// other numeric type here.
    pub fn rem(&self, other: &Exact) -> Result<Exact, Trouble> {
        if other.is_zero() {
            return Err(Trouble::DivideByZero);
        }
        let quotient = self.div(other)?;
        let whole = Exact::whole(quotient.floor());
        Ok(self.sub(&other.mul(&whole)))
    }

    /// Raising to a power, which stays rational only for whole exponents.
    pub fn pow(&self, exponent: &Exact) -> Result<Exact, Trouble> {
        if !exponent.denominator.is_one() {
            return Err(Trouble::FractionalPower);
        }
        let magnitude = exponent.numerator.magnitude();
        let Some(times) = magnitude.to_u64().filter(|n| *n <= Self::POWER_LIMIT) else {
            return Err(Trouble::PowerTooLarge);
        };

        let mut out = Exact::one();
        // Square and multiply, so a thousand is ten multiplications and not a thousand.
        let mut base = self.clone();
        let mut left = times;
        while left > 0 {
            if left & 1 == 1 {
                out = out.mul(&base);
            }
            left >>= 1;
            if left > 0 {
                base = base.mul(&base);
            }
        }

        if exponent.numerator.is_negative() {
            if out.is_zero() {
                return Err(Trouble::DivideByZero);
            }
            return Exact::one().div(&out);
        }
        Ok(out)
    }

    /// Read one as it is written: `3`, `-2.5`, or `1/3`.
    ///
    /// The fraction form is there because otherwise a third could not be written down at
    /// all — and a type whose whole purpose is exactness should not make you approximate
    /// the first interesting number.
    pub fn parse(text: &str) -> Option<Exact> {
        let text = text.trim();
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };

        let value = if let Some((top, bottom)) = rest.split_once('/') {
            Exact::ratio(Big::from_digits(top.trim())?, Big::from_digits(bottom.trim())?)?
        } else if let Some((whole, fraction)) = rest.split_once('.') {
            // `12.34` is 1234 over a hundred. Nothing is lost, because nothing is rounded.
            let whole = if whole.is_empty() { Big::zero() } else { Big::from_digits(whole)? };
            let fraction = if fraction.is_empty() { "0" } else { fraction };
            let places = fraction.len() as u32;
            let scale = Big::ten_to(places);
            let numerator = whole.mul(&scale).add(&Big::from_digits(fraction)?);
            Exact::ratio(numerator, scale)?
        } else {
            Exact::whole(Big::from_digits(rest)?)
        };

        Some(if negative { value.negated() } else { value })
    }
}

impl Ord for Exact {
    fn cmp(&self, other: &Self) -> Ordering {
        // Both denominators are positive, so multiplying across keeps the direction.
        self.numerator
            .mul(&other.denominator)
            .cmp(&other.numerator.mul(&self.denominator))
    }
}

impl PartialOrd for Exact {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Exact {
    /// Written as a fraction, because that is what it is. A third has no finite decimal
    /// and printing `0.333…` would be the one thing this type exists not to do.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.denominator.is_one() {
            return write!(f, "{}", self.numerator);
        }
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn er(text: &str) -> Exact {
        Exact::parse(text).unwrap_or_else(|| panic!("`{text}` should read"))
    }

    #[test]
    fn what_floats_get_wrong_this_gets_right() {
        // The example every article about floating point opens with.
        assert_eq!(er("0.1").add(&er("0.2")).to_string(), "3/10");
        // And a third is a third, rather than the nearest thing to one.
        let third = er("1/3");
        assert_eq!(third.add(&third).add(&third).to_string(), "1");
    }

    #[test]
    fn a_value_is_always_in_lowest_terms_with_the_sign_on_top() {
        assert_eq!(er("2/4").to_string(), "1/2");
        assert_eq!(er("-2/4").to_string(), "-1/2");
        assert_eq!(er("6/3").to_string(), "2");
        assert_eq!(er("0.50").to_string(), "1/2");
        assert_eq!(er("0/5").to_string(), "0");
        // A negative denominator moves its sign to the numerator, so there is one shape.
        assert_eq!(Exact::ratio(Big::from_i64(1), Big::from_i64(-2)).unwrap().to_string(), "-1/2");
    }

    #[test]
    fn the_four_operations_are_what_a_fraction_says_they_are() {
        let (a, b) = (er("3/4"), er("2/3"));
        assert_eq!(a.add(&b).to_string(), "17/12");
        assert_eq!(a.sub(&b).to_string(), "1/12");
        assert_eq!(a.mul(&b).to_string(), "1/2");
        assert_eq!(a.div(&b).expect("not by zero").to_string(), "9/8");
    }

    #[test]
    fn dividing_by_nothing_is_told_about_rather_than_guessed_at() {
        assert_eq!(er("1").div(&er("0")), Err(Trouble::DivideByZero));
        assert_eq!(er("1").rem(&er("0")), Err(Trouble::DivideByZero));
    }

    #[test]
    fn the_remainder_is_floored_like_every_other_type_here() {
        // Floored, so the answer takes the sign of the divisor.
        assert_eq!(er("7").rem(&er("3")).expect("not by zero").to_string(), "1");
        assert_eq!(er("-7").rem(&er("3")).expect("not by zero").to_string(), "2");
        assert_eq!(er("7").rem(&er("-3")).expect("not by zero").to_string(), "-2");
        assert_eq!(er("-7").rem(&er("-3")).expect("not by zero").to_string(), "-1");
        // And it works on fractions, where truncation would have nothing to truncate.
        assert_eq!(er("7/2").rem(&er("1/3")).expect("not by zero").to_string(), "1/6");
    }

    #[test]
    fn flooring_rounds_down_rather_than_towards_nothing() {
        assert_eq!(er("7/2").floor().to_string(), "3");
        assert_eq!(er("-7/2").floor().to_string(), "-4");
        assert_eq!(er("4").floor().to_string(), "4");
        assert_eq!(er("-4").floor().to_string(), "-4");
    }

    #[test]
    fn a_power_has_to_be_a_whole_number() {
        assert_eq!(er("2/3").pow(&er("3")).expect("whole").to_string(), "8/27");
        assert_eq!(er("2").pow(&er("0")).expect("whole").to_string(), "1");
        assert_eq!(er("2").pow(&er("-2")).expect("whole").to_string(), "1/4");
        // The square root of two is not a ratio, and this type only holds ratios.
        assert_eq!(er("2").pow(&er("1/2")), Err(Trouble::FractionalPower));
        assert_eq!(er("0").pow(&er("-1")), Err(Trouble::DivideByZero));
        // And one big enough to be an afternoon is refused rather than attempted.
        assert_eq!(er("2").pow(&er("100000")), Err(Trouble::PowerTooLarge));
        // A large but allowed one still comes out right.
        assert_eq!(er("2").pow(&er("64")).expect("whole").to_string(), "18446744073709551616");
    }

    #[test]
    fn two_of_them_order_the_way_the_numbers_do() {
        assert!(er("1/3") < er("1/2"));
        assert!(er("-1/3") > er("-1/2"));
        assert!(er("2/4") == er("1/2"));
        assert!(er("0") > er("-1/1000000000000000000000000"));
    }

    #[test]
    fn a_written_value_reads_the_way_it_looks() {
        assert_eq!(er("3").to_string(), "3");
        assert_eq!(er("-2.5").to_string(), "-5/2");
        assert_eq!(er(".5").to_string(), "1/2");
        assert_eq!(er("2.").to_string(), "2");
        assert_eq!(er("1/3").to_string(), "1/3");
        assert_eq!(er("-1/3").to_string(), "-1/3");
        assert!(Exact::parse("").is_none());
        assert!(Exact::parse("1/0").is_none());
        assert!(Exact::parse("hello").is_none());
        assert!(Exact::parse("1.2.3").is_none());
    }

    #[test]
    fn nothing_here_overflows() {
        // Twenty halvings of a number no fixed width could hold, and back again.
        let big = er("123456789012345678901234567890123456789");
        let half = er("1/2");
        let mut small = big.clone();
        for _ in 0..20 {
            small = small.mul(&half);
        }
        for _ in 0..20 {
            small = small.div(&half).expect("not by zero");
        }
        assert_eq!(small, big);
    }
}

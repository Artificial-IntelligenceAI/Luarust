//! Fixed-width unsigned integers, as wide as significand arithmetic asks for.
//!
//! Every float format here does its arithmetic on an integer significand, and those
//! stop fitting in `u128` as soon as `b128` and `b256` are in play: multiplying two
//! 237-bit `b256` significands needs 474 bits of room, and dividing to a rounded
//! quotient needs about as many again. [`Uint<N>`] is `N` 64-bit limbs, little-endian,
//! carrying the operations soft-float actually asks for and nothing else.
//!
//! Nothing here is clever. Division is bit-by-bit long division — `O(bits)` where a
//! limb-wise algorithm would be `O(limbs²)` — because it is short enough to see that
//! it is right, and it sits behind a function that can be replaced the day a benchmark
//! says it matters.
//!
//! The tests check the narrow widths against `u64` and `u128`, which the machine
//! already knows how to do correctly. That only reaches `N = 1` and `N = 2`; the wider
//! widths run the same code paths with more limbs, so what those tests really pin down
//! is limb carry propagation, which is where the bugs live.

// Several loops here index two or three arrays at once, or index one at an offset from
// the loop variable. Written as iterators they would hide the limb arithmetic rather
// than clarify it.
#![allow(clippy::needless_range_loop)]

use core::cmp::Ordering;
use core::fmt;

/// An `N`-limb unsigned integer. Limbs are little-endian: `limbs[0]` is least significant.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Uint<const N: usize> {
    limbs: [u64; N],
}

impl<const N: usize> Uint<N> {
    /// Width in bits.
    pub const BITS: u32 = (N as u32) * 64;

    /// Zero.
    pub const ZERO: Self = Self { limbs: [0; N] };

    /// The largest representable value: every bit set.
    pub const MAX: Self = Self { limbs: [u64::MAX; N] };

    /// Build from limbs, least significant first.
    pub fn from_limbs(limbs: [u64; N]) -> Self {
        Self { limbs }
    }

    /// The limbs, least significant first.
    pub fn limbs(&self) -> &[u64; N] {
        &self.limbs
    }

    /// Build from a `u64`, zero-extended.
    pub fn from_u64(v: u64) -> Self {
        let mut limbs = [0u64; N];
        limbs[0] = v;
        Self { limbs }
    }

    /// Build from a `u128`, zero-extended. The high half is dropped when `N == 1`.
    pub fn from_u128(v: u128) -> Self {
        let mut limbs = [0u64; N];
        limbs[0] = v as u64;
        if N > 1 {
            limbs[1] = (v >> 64) as u64;
        }
        Self { limbs }
    }

    /// The low 64 bits.
    pub fn low64(&self) -> u64 {
        self.limbs[0]
    }

    /// The low 128 bits.
    pub fn low128(&self) -> u128 {
        let hi = if N > 1 { self.limbs[1] as u128 } else { 0 };
        (hi << 64) | (self.limbs[0] as u128)
    }

    /// Whether every bit is clear.
    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// Whether bit 0 is set.
    pub fn is_odd(&self) -> bool {
        self.limbs[0] & 1 == 1
    }

    /// Bit `i`, counting from the least significant. Bits at or past [`Self::BITS`] read as clear.
    pub fn bit(&self, i: u32) -> bool {
        if i >= Self::BITS {
            return false;
        }
        (self.limbs[(i / 64) as usize] >> (i % 64)) & 1 == 1
    }

    /// Set bit `i`. Out-of-range indices are ignored.
    pub fn set_bit(&mut self, i: u32) {
        if i < Self::BITS {
            self.limbs[(i / 64) as usize] |= 1u64 << (i % 64);
        }
    }

    /// Clear bit `i`. Out-of-range indices are ignored.
    pub fn clear_bit(&mut self, i: u32) {
        if i < Self::BITS {
            self.limbs[(i / 64) as usize] &= !(1u64 << (i % 64));
        }
    }

    /// One past the highest set bit, or zero for zero. The bit length of the value.
    pub fn bit_len(&self) -> u32 {
        for i in (0..N).rev() {
            if self.limbs[i] != 0 {
                return (i as u32) * 64 + (64 - self.limbs[i].leading_zeros());
            }
        }
        0
    }

    /// Clear bits above the value, counted from the top of the width.
    pub fn leading_zeros(&self) -> u32 {
        Self::BITS - self.bit_len()
    }

    /// Whether any bit below position `k` is set.
    ///
    /// This is the sticky bit: when a rounding step drops the low `k` bits of a value,
    /// the result is a tie only if every one of them was clear.
    pub fn low_bits_any(&self, k: u32) -> bool {
        if k == 0 {
            return false;
        }
        let k = k.min(Self::BITS);
        let full = (k / 64) as usize;
        if self.limbs[..full].iter().any(|&l| l != 0) {
            return true;
        }
        let rest = k % 64;
        rest > 0 && full < N && self.limbs[full] & ((1u64 << rest) - 1) != 0
    }

    /// Add, reporting whether the sum carried out of the width.
    pub fn overflowing_add(self, rhs: Self) -> (Self, bool) {
        let mut out = [0u64; N];
        let mut carry = 0u64;
        for i in 0..N {
            // Both additions cannot carry: if the first wraps, its result is at most
            // `2^64 - 2`, so adding one more cannot reach `2^64`.
            let (a, c1) = self.limbs[i].overflowing_add(rhs.limbs[i]);
            let (b, c2) = a.overflowing_add(carry);
            out[i] = b;
            carry = (c1 | c2) as u64;
        }
        (Self { limbs: out }, carry != 0)
    }

    /// Subtract, reporting whether the difference borrowed past the width.
    pub fn overflowing_sub(self, rhs: Self) -> (Self, bool) {
        let mut out = [0u64; N];
        let mut borrow = 0u64;
        for i in 0..N {
            let (a, b1) = self.limbs[i].overflowing_sub(rhs.limbs[i]);
            let (b, b2) = a.overflowing_sub(borrow);
            out[i] = b;
            borrow = (b1 | b2) as u64;
        }
        (Self { limbs: out }, borrow != 0)
    }

    /// Add, discarding any carry out of the width.
    pub fn wrapping_add(self, rhs: Self) -> Self {
        self.overflowing_add(rhs).0
    }

    /// Subtract, wrapping past zero.
    pub fn wrapping_sub(self, rhs: Self) -> Self {
        self.overflowing_sub(rhs).0
    }

    /// Add, or `None` if the sum does not fit.
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        let (v, carry) = self.overflowing_add(rhs);
        if carry { None } else { Some(v) }
    }

    /// Subtract, or `None` if `rhs` is the larger.
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        let (v, borrow) = self.overflowing_sub(rhs);
        if borrow { None } else { Some(v) }
    }

    /// Shift left, discarding bits shifted past the width.
    ///
    /// Deliberately not `std::ops::Shl`: shifting a fixed-width integer by its own width
    /// or more is conventionally a panic, and here it yields zero, because that is what
    /// a rounding step wants when it shifts a significand entirely out of range.
    #[allow(clippy::should_implement_trait)]
    pub fn shl(self, n: u32) -> Self {
        if n >= Self::BITS {
            return Self::ZERO;
        }
        let limb = (n / 64) as usize;
        let bit = n % 64;
        let mut out = [0u64; N];
        for i in (limb..N).rev() {
            let src = i - limb;
            out[i] = if bit == 0 {
                self.limbs[src]
            } else {
                let lo = self.limbs[src] << bit;
                let hi = if src > 0 { self.limbs[src - 1] >> (64 - bit) } else { 0 };
                lo | hi
            };
        }
        Self { limbs: out }
    }

    /// Shift right, discarding bits shifted past zero. Saturates to zero, as [`Self::shl`] does.
    #[allow(clippy::should_implement_trait)]
    pub fn shr(self, n: u32) -> Self {
        if n >= Self::BITS {
            return Self::ZERO;
        }
        let limb = (n / 64) as usize;
        let bit = n % 64;
        let mut out = [0u64; N];
        for i in 0..(N - limb) {
            let src = i + limb;
            out[i] = if bit == 0 {
                self.limbs[src]
            } else {
                let hi = self.limbs[src] >> bit;
                let lo = if src + 1 < N { self.limbs[src + 1] << (64 - bit) } else { 0 };
                hi | lo
            };
        }
        Self { limbs: out }
    }

    /// The full `2N`-limb product, as `(low, high)`.
    ///
    /// Nothing is lost: a product of two `N`-limb values always fits in `2N` limbs,
    /// which is why multiplication is spelled this way rather than as a wrapping op.
    pub fn mul_wide(self, rhs: Self) -> (Self, Self) {
        let mut lo = [0u64; N];
        let mut hi = [0u64; N];

        // `at`/`put` address the product as one flat 2N-limb number.
        macro_rules! at {
            ($idx:expr) => {
                if $idx < N { lo[$idx] } else { hi[$idx - N] }
            };
        }
        macro_rules! put {
            ($idx:expr, $val:expr) => {
                if $idx < N { lo[$idx] = $val } else { hi[$idx - N] = $val }
            };
        }

        for i in 0..N {
            if self.limbs[i] == 0 {
                continue;
            }
            let mut carry = 0u64;
            for j in 0..N {
                let idx = i + j;
                let t = (self.limbs[i] as u128) * (rhs.limbs[j] as u128)
                    + (at!(idx) as u128)
                    + (carry as u128);
                put!(idx, t as u64);
                carry = (t >> 64) as u64;
            }
            let mut idx = i + N;
            while carry != 0 && idx < 2 * N {
                let t = (at!(idx) as u128) + (carry as u128);
                put!(idx, t as u64);
                carry = (t >> 64) as u64;
                idx += 1;
            }
        }

        (Self { limbs: lo }, Self { limbs: hi })
    }

    /// The low `N` limbs of the product, discarding overflow.
    pub fn wrapping_mul(self, rhs: Self) -> Self {
        self.mul_wide(rhs).0
    }

    /// Quotient and remainder.
    ///
    /// # Panics
    ///
    /// If `rhs` is zero. Division by zero is a decision the caller has to make — the
    /// float formats answer it with an infinity or a NaN, and `er` answers it with an
    /// error — so it is not this layer's to guess.
    pub fn div_rem(self, rhs: Self) -> (Self, Self) {
        assert!(!rhs.is_zero(), "Uint::div_rem: divide by zero");

        if self < rhs {
            return (Self::ZERO, self);
        }

        // The loop below keeps a running remainder below `rhs` and doubles it each step,
        // which needs `2 * rhs - 1` to stay inside the width. When `rhs` has its top bit
        // set that is not true — but then `rhs > 2^(BITS-1)` and the dividend is under
        // `2^BITS`, so the quotient can only be one.
        if rhs.bit_len() == Self::BITS {
            return (Self::from_u64(1), self.wrapping_sub(rhs));
        }

        let mut quot = Self::ZERO;
        let mut rem = Self::ZERO;
        for i in (0..self.bit_len()).rev() {
            rem = rem.shl(1);
            if self.bit(i) {
                rem.set_bit(0);
            }
            if rem >= rhs {
                rem = rem.wrapping_sub(rhs);
                quot.set_bit(i);
            }
        }
        (quot, rem)
    }

    /// A value with the low `k` bits set and nothing else.
    pub fn low_mask(k: u32) -> Self {
        if k == 0 {
            return Self::ZERO;
        }
        if k >= Self::BITS {
            return Self::MAX;
        }
        Self::from_u64(1).shl(k).wrapping_sub(Self::from_u64(1))
    }

    /// Reinterpret at a different width: zero-extending when widening, truncating when narrowing.
    pub fn resize<const M: usize>(self) -> Uint<M> {
        let mut out = [0u64; M];
        let k = if N < M { N } else { M };
        out[..k].copy_from_slice(&self.limbs[..k]);
        Uint { limbs: out }
    }
}

impl<const N: usize> core::ops::BitAnd for Uint<N> {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        let mut out = [0u64; N];
        for i in 0..N {
            out[i] = self.limbs[i] & rhs.limbs[i];
        }
        Self { limbs: out }
    }
}

impl<const N: usize> core::ops::BitOr for Uint<N> {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        let mut out = [0u64; N];
        for i in 0..N {
            out[i] = self.limbs[i] | rhs.limbs[i];
        }
        Self { limbs: out }
    }
}

impl<const N: usize> core::ops::BitXor for Uint<N> {
    type Output = Self;
    fn bitxor(self, rhs: Self) -> Self {
        let mut out = [0u64; N];
        for i in 0..N {
            out[i] = self.limbs[i] ^ rhs.limbs[i];
        }
        Self { limbs: out }
    }
}

impl<const N: usize> core::ops::Not for Uint<N> {
    type Output = Self;
    fn not(self) -> Self {
        let mut out = [0u64; N];
        for i in 0..N {
            out[i] = !self.limbs[i];
        }
        Self { limbs: out }
    }
}

impl<const N: usize> Default for Uint<N> {
    fn default() -> Self {
        Self::ZERO
    }
}

impl<const N: usize> Ord for Uint<N> {
    fn cmp(&self, other: &Self) -> Ordering {
        for i in (0..N).rev() {
            match self.limbs[i].cmp(&other.limbs[i]) {
                Ordering::Equal => {}
                unequal => return unequal,
            }
        }
        Ordering::Equal
    }
}

impl<const N: usize> PartialOrd for Uint<N> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<const N: usize> fmt::Debug for Uint<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x")?;
        for i in (0..N).rev() {
            write!(f, "{:016x}", self.limbs[i])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64*, so the "random" cases are the same on every run and a failure can
    /// be reproduced by reading the seed off the test rather than off a log.
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
    fn u64_roundtrip() {
        for v in [0u64, 1, 42, u64::MAX] {
            assert_eq!(Uint::<4>::from_u64(v).low64(), v);
        }
    }

    #[test]
    fn u128_roundtrip() {
        let mut rng = Rng::new(1);
        for _ in 0..1000 {
            let v = ((rng.next() as u128) << 64) | rng.next() as u128;
            assert_eq!(Uint::<4>::from_u128(v).low128(), v);
        }
    }

    #[test]
    fn add_matches_u128() {
        let mut rng = Rng::new(2);
        for _ in 0..10_000 {
            let a = ((rng.next() as u128) << 64) | rng.next() as u128;
            let b = ((rng.next() as u128) << 64) | rng.next() as u128;
            let (sum, carry) = Uint::<2>::from_u128(a).overflowing_add(Uint::<2>::from_u128(b));
            assert_eq!(sum.low128(), a.wrapping_add(b));
            assert_eq!(carry, a.checked_add(b).is_none());
        }
    }

    #[test]
    fn sub_matches_u128() {
        let mut rng = Rng::new(3);
        for _ in 0..10_000 {
            let a = ((rng.next() as u128) << 64) | rng.next() as u128;
            let b = ((rng.next() as u128) << 64) | rng.next() as u128;
            let (diff, borrow) = Uint::<2>::from_u128(a).overflowing_sub(Uint::<2>::from_u128(b));
            assert_eq!(diff.low128(), a.wrapping_sub(b));
            assert_eq!(borrow, a.checked_sub(b).is_none());
        }
    }

    #[test]
    fn carry_propagates_across_every_limb() {
        // The one case limb-wise addition gets wrong if a carry is dropped.
        let (sum, carry) = Uint::<4>::MAX.overflowing_add(Uint::<4>::from_u64(1));
        assert!(carry);
        assert_eq!(sum, Uint::<4>::ZERO);

        let (diff, borrow) = Uint::<4>::ZERO.overflowing_sub(Uint::<4>::from_u64(1));
        assert!(borrow);
        assert_eq!(diff, Uint::<4>::MAX);
    }

    #[test]
    fn mul_matches_u128() {
        let mut rng = Rng::new(4);
        for _ in 0..10_000 {
            let a = rng.next();
            let b = rng.next();
            let (lo, hi) = Uint::<1>::from_u64(a).mul_wide(Uint::<1>::from_u64(b));
            let want = (a as u128) * (b as u128);
            assert_eq!(lo.low64(), want as u64);
            assert_eq!(hi.low64(), (want >> 64) as u64);
        }
    }

    #[test]
    fn wide_mul_matches_u128() {
        // Two-limb inputs whose product still fits in 128 bits, so `u128` can judge it.
        let mut rng = Rng::new(5);
        for _ in 0..10_000 {
            let a = rng.next() >> 1;
            let b = rng.next() >> 1;
            let (lo, hi) = Uint::<2>::from_u64(a).mul_wide(Uint::<2>::from_u64(b));
            assert!(hi.is_zero());
            assert_eq!(lo.low128(), (a as u128) * (b as u128));
        }
    }

    #[test]
    fn mul_max_squared() {
        let (lo, hi) = Uint::<2>::MAX.mul_wide(Uint::<2>::MAX);
        // (2^128 - 1)^2 = 2^256 - 2^129 + 1
        assert_eq!(lo.low128(), 1);
        assert_eq!(hi.low128(), u128::MAX - 1);
    }

    #[test]
    fn div_matches_u128() {
        let mut rng = Rng::new(6);
        for _ in 0..5_000 {
            let a = ((rng.next() as u128) << 64) | rng.next() as u128;
            let b = match ((rng.next() as u128) << 64) | rng.next() as u128 {
                0 => 1,
                v => v,
            };
            let (q, r) = Uint::<2>::from_u128(a).div_rem(Uint::<2>::from_u128(b));
            assert_eq!(q.low128(), a / b, "{a} / {b}");
            assert_eq!(r.low128(), a % b, "{a} % {b}");
        }
    }

    #[test]
    fn div_by_small_matches_u128() {
        // Small divisors take the long-division path for many more iterations.
        let mut rng = Rng::new(7);
        for _ in 0..5_000 {
            let a = ((rng.next() as u128) << 64) | rng.next() as u128;
            let b = (rng.next() % 1000 + 1) as u128;
            let (q, r) = Uint::<2>::from_u128(a).div_rem(Uint::<2>::from_u128(b));
            assert_eq!(q.low128(), a / b);
            assert_eq!(r.low128(), a % b);
        }
    }

    #[test]
    fn div_by_top_bit_divisor() {
        // The special case: a divisor above half the width, where the quotient is 0 or 1.
        let big = Uint::<2>::MAX;
        let (q, r) = big.div_rem(big);
        assert_eq!(q.low128(), 1);
        assert!(r.is_zero());

        let (q, r) = Uint::<2>::from_u128(u128::MAX).div_rem(Uint::<2>::from_u128(1 << 127));
        assert_eq!(q.low128(), 1);
        assert_eq!(r.low128(), u128::MAX - (1 << 127));

        let (q, r) = Uint::<2>::from_u128((1 << 127) - 1).div_rem(Uint::<2>::from_u128(1 << 127));
        assert!(q.is_zero());
        assert_eq!(r.low128(), (1 << 127) - 1);
    }

    #[test]
    #[should_panic(expected = "divide by zero")]
    fn div_by_zero_panics() {
        let _ = Uint::<2>::from_u64(1).div_rem(Uint::<2>::ZERO);
    }

    #[test]
    fn shifts_match_u128() {
        let mut rng = Rng::new(8);
        for _ in 0..2_000 {
            let v = ((rng.next() as u128) << 64) | rng.next() as u128;
            let u = Uint::<2>::from_u128(v);
            for n in 0..=128u32 {
                let want_l = if n >= 128 { 0 } else { v << n };
                let want_r = if n >= 128 { 0 } else { v >> n };
                assert_eq!(u.shl(n).low128(), want_l, "{v} << {n}");
                assert_eq!(u.shr(n).low128(), want_r, "{v} >> {n}");
            }
        }
    }

    #[test]
    fn shifts_cross_limbs_at_wide_widths() {
        let mut v = Uint::<4>::ZERO;
        v.set_bit(0);
        for n in 0..256u32 {
            let shifted = v.shl(n);
            assert_eq!(shifted.bit_len(), n + 1);
            assert!(shifted.bit(n));
            assert_eq!(shifted.shr(n), v);
        }
        assert!(v.shl(256).is_zero());
    }

    #[test]
    fn bit_len_matches_u128() {
        let mut rng = Rng::new(9);
        for _ in 0..10_000 {
            let v = ((rng.next() as u128) << 64) | rng.next() as u128;
            let want = 128 - v.leading_zeros();
            assert_eq!(Uint::<2>::from_u128(v).bit_len(), want);
        }
        assert_eq!(Uint::<4>::ZERO.bit_len(), 0);
    }

    #[test]
    fn sticky_bit() {
        let mut v = Uint::<2>::ZERO;
        v.set_bit(70);
        assert!(!v.low_bits_any(70));
        assert!(v.low_bits_any(71));
        assert!(v.low_bits_any(128));
        assert!(!Uint::<2>::ZERO.low_bits_any(128));
        assert!(!v.low_bits_any(0));
    }

    #[test]
    fn ordering_is_by_value() {
        let mut rng = Rng::new(10);
        for _ in 0..10_000 {
            let a = ((rng.next() as u128) << 64) | rng.next() as u128;
            let b = ((rng.next() as u128) << 64) | rng.next() as u128;
            assert_eq!(
                Uint::<2>::from_u128(a).cmp(&Uint::<2>::from_u128(b)),
                a.cmp(&b)
            );
        }
    }

    #[test]
    fn bitwise_ops_match_u128() {
        let mut rng = Rng::new(11);
        for _ in 0..10_000 {
            let a = ((rng.next() as u128) << 64) | rng.next() as u128;
            let b = ((rng.next() as u128) << 64) | rng.next() as u128;
            let (ua, ub) = (Uint::<2>::from_u128(a), Uint::<2>::from_u128(b));
            assert_eq!((ua & ub).low128(), a & b);
            assert_eq!((ua | ub).low128(), a | b);
            assert_eq!((ua ^ ub).low128(), a ^ b);
            assert_eq!((!ua).low128(), !a);
        }
    }

    #[test]
    fn low_mask_matches_u128() {
        for k in 0..=128u32 {
            let want = if k == 0 {
                0
            } else if k >= 128 {
                u128::MAX
            } else {
                (1u128 << k) - 1
            };
            assert_eq!(Uint::<2>::low_mask(k).low128(), want, "k = {k}");
        }
        assert_eq!(Uint::<4>::low_mask(200).bit_len(), 200);
    }

    #[test]
    fn resize_widens_and_narrows() {
        let v = Uint::<2>::from_u128(u128::MAX);
        assert_eq!(v.resize::<4>().low128(), u128::MAX);
        assert!(v.resize::<4>().resize::<2>() == v);
        assert_eq!(v.resize::<1>().low64(), u64::MAX);
    }
}

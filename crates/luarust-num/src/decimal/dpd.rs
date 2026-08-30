//! The other way of writing a decimal significand down.
//!
//! **DPD** — densely packed decimal — squeezes three decimal digits into ten bits, which
//! is as tight as three digits go: a thousand values need ten bits and 2^10 is 1024. BID
//! instead keeps the significand as an ordinary binary integer, which is looser in the
//! encoding and very much easier to compute with.
//!
//! Nothing here is arithmetic. A number means the same thing in both encodings; this only
//! changes the pattern it is written as, at the edge, when a `Luarust.toml` says to. That
//! is why it costs nothing at run time: it is a pack on the way out and an unpack on the
//! way in, and never anything in between.
//!
//! The bit shuffling below is the table from the standard, written out as the conditions
//! it actually is. It is not pretty and it is not supposed to be — it is a fixed mapping
//! that either matches the standard or does not, and the tests say which.

use super::Format;
use crate::Uint;

/// Three digits into ten bits.
///
/// The table is the standard's, written out as it stands. Name the three digits' bits
/// `abcd`, `efgh` and `ijkm`, and the ten output bits `pqr stu v wxy`; then which of `a`,
/// `e` and `i` are set — that is, which digits are eight or nine — picks the row.
pub fn encode_declet(digits: u32) -> u32 {
    let (d1, d2, d3) = (digits / 100 % 10, digits / 10 % 10, digits % 10);
    let (a, b, c, d) = (d1 >> 3 & 1, d1 >> 2 & 1, d1 >> 1 & 1, d1 & 1);
    let (e, f, g, h) = (d2 >> 3 & 1, d2 >> 2 & 1, d2 >> 1 & 1, d2 & 1);
    let (i, j, k, m) = (d3 >> 3 & 1, d3 >> 2 & 1, d3 >> 1 & 1, d3 & 1);

    let (pqr, stu, v, wxy) = match (a, e, i) {
        (0, 0, 0) => (b << 2 | c << 1 | d, f << 2 | g << 1 | h, 0, j << 2 | k << 1 | m),
        (0, 0, _) => (b << 2 | c << 1 | d, f << 2 | g << 1 | h, 1, m),
        (0, _, 0) => (b << 2 | c << 1 | d, j << 2 | k << 1 | h, 1, 0b010 | m),
        (0, _, _) => (b << 2 | c << 1 | d, 0b100 | h, 1, 0b110 | m),
        (_, 0, 0) => (j << 2 | k << 1 | d, f << 2 | g << 1 | h, 1, 0b100 | m),
        (_, 0, _) => (f << 2 | g << 1 | d, 0b010 | h, 1, 0b110 | m),
        (_, _, 0) => (j << 2 | k << 1 | d, h, 1, 0b110 | m),
        (_, _, _) => (d, 0b110 | h, 1, 0b110 | m),
    };
    pqr << 7 | stu << 4 | v << 3 | wxy
}

/// Ten bits back into three digits, which is the table above read the other way.
pub fn decode_declet(bits: u32) -> u32 {
    let at = |n: u32| (bits >> n) & 1;
    let (p, q, r) = (at(9), at(8), at(7));
    let (s, t, u) = (at(6), at(5), at(4));
    let (v, w, x, y) = (at(3), at(2), at(1), at(0));
    let pqr = p << 2 | q << 1 | r;
    let stu = s << 2 | t << 1 | u;

    let (d1, d2, d3) = if v == 0 {
        (pqr, stu, w << 2 | x << 1 | y)
    } else {
        match (w, x) {
            (0, 0) => (pqr, stu, 8 + y),
            (0, _) => (pqr, 8 + u, s << 2 | t << 1 | y),
            (_, 0) => (8 + r, stu, p << 2 | q << 1 | y),
            // All three of the remaining shapes are told apart by `s` and `t`.
            _ => match (s, t) {
                (0, 0) => (8 + r, 8 + u, p << 2 | q << 1 | y),
                (0, _) => (8 + r, p << 2 | q << 1 | u, 8 + y),
                (_, 0) => (pqr, 8 + u, 8 + y),
                _ => (8 + r, 8 + u, 8 + y),
            },
        }
    };
    d1 * 100 + d2 * 10 + d3
}

/// How many declets a format's trailing field holds.
fn declets(fmt: Format) -> u32 {
    fmt.trailing_bits / 10
}

/// The trailing digits, as declets.
pub fn pack_trailing<const W: usize>(fmt: Format, mut rest: Uint<W>) -> Uint<W> {
    let thousand = Uint::<W>::from_u64(1000);
    let mut out = Uint::ZERO;
    for slot in 0..declets(fmt) {
        let (next, three) = rest.div_rem(thousand);
        rest = next;
        out = out | Uint::from_u64(u64::from(encode_declet(three.low64() as u32))).shl(slot * 10);
    }
    out
}

/// Declets back into a plain integer.
pub fn unpack_trailing<const W: usize>(fmt: Format, packed: Uint<W>) -> Uint<W> {
    let mut out = Uint::<W>::ZERO;
    let thousand = Uint::<W>::from_u64(1000);
    for slot in (0..declets(fmt)).rev() {
        let bits = (packed.shr(slot * 10).low64() & 0x3ff) as u32;
        out = out.wrapping_mul(thousand).wrapping_add(Uint::from_u64(u64::from(decode_declet(bits))));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_three_digits_survives_the_round_trip() {
        // A thousand values into ten bits and back. Anything the standard's table gets
        // wrong shows up here, because there is nowhere for a mistake to hide.
        for digits in 0..1000u32 {
            let encoded = encode_declet(digits);
            assert!(encoded < 1024, "{digits} encoded to {encoded}, which needs eleven bits");
            assert_eq!(decode_declet(encoded), digits, "{digits}");
        }
    }

    #[test]
    fn the_declets_the_standard_names_are_the_ones_produced() {
        // Worked out by hand from the table, so a plausible-looking mistake in the shifts
        // above has something to disagree with.
        assert_eq!(encode_declet(0), 0b00_0000_0000);
        assert_eq!(encode_declet(9), 0b00_0000_1001);
        assert_eq!(encode_declet(5), 0b00_0000_0101);
        assert_eq!(encode_declet(77), 0b00_0111_0111);
        // 999 is `abcd efgh ijkm` all 1001, so the last row of the table: pqr = d = 1,
        // stu = 110|h = 111, v = 1, wxy = 110|m = 111.
        assert_eq!(encode_declet(999), 0b00_1111_1111);
        // 888 is the same row with every low bit clear.
        assert_eq!(encode_declet(888), 0b00_0110_1110);
    }
}

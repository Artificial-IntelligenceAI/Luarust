//! Grapheme clusters, and how wide they draw.
//!
//! A "character" as a reader counts them is a **grapheme cluster** — one or more Unicode
//! scalars that combine into a single thing on the page. `é` written as `e` plus a
//! combining acute is one. A flag is one. `🧑‍🧑‍🧒‍🧒` is one, though it is seven scalars
//! welded together with zero-width joiners and twenty-five bytes on disk.
//!
//! Luarust cares because names may contain anything you can type, so an error column has
//! to count the way the person writing the name counts, or it points at the wrong place.
//!
//! This is a **subset of UAX #29**, not the whole of it: the rules for joining, marks,
//! flags, Hangul and emoji sequences are here, and the property tables are ranges chosen
//! to cover what appears in real source rather than generated from the full Unicode
//! database. Prepend and SpacingMark, which matter for a handful of Indic scripts, are
//! not handled. Where it is wrong it is wrong by splitting a cluster that should have
//! held together, which moves a caret rather than crashing anything.

/// Zero-width joiner: the glue in `🧑‍🧑‍🧒‍🧒`.
const ZWJ: char = '\u{200D}';

/// Whether the scalar continues whatever came before it rather than starting something.
///
/// Combining marks, variation selectors, skin-tone modifiers and the like.
fn is_extend(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F     // combining diacritics
        | 0x0483..=0x0489
        | 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7
        | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670
        | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED
        | 0x0711 | 0x0730..=0x074A | 0x07A6..=0x07B0 | 0x07EB..=0x07F3
        | 0x0816..=0x0819 | 0x081B..=0x0823 | 0x0825..=0x0827 | 0x0829..=0x082D
        | 0x0900..=0x0902 | 0x093A | 0x093C | 0x0941..=0x0948 | 0x094D
        | 0x0951..=0x0957 | 0x0962..=0x0963
        | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E   // Thai
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF            // more combining
        | 0x200B..=0x200F                              // zero-width space, ZWNJ, ZWJ, marks
        | 0x20D0..=0x20F0                              // combining for symbols
        | 0xFE00..=0xFE0F                              // variation selectors
        | 0xFE20..=0xFE2F                              // combining half marks
        | 0x1F3FB..=0x1F3FF                            // skin tone modifiers
        | 0xE0020..=0xE007F                            // tag characters, for subdivision flags
        | 0xE0100..=0xE01EF                            // variation selectors, supplement
    )
}

/// One half of a flag. Two in a row make one.
fn is_regional_indicator(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

/// Emoji and the older symbols that behave like them when joined.
fn is_pictographic(c: char) -> bool {
    matches!(c as u32,
        0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139
        | 0x2194..=0x21AA
        | 0x231A..=0x231B | 0x2328 | 0x23CF..=0x23FA
        | 0x24C2 | 0x25AA..=0x25FE
        | 0x2600..=0x27BF                              // misc symbols and dingbats
        | 0x2934..=0x2935 | 0x2B00..=0x2BFF
        | 0x3030 | 0x303D | 0x3297 | 0x3299
        | 0x1F000..=0x1FAFF                            // every modern emoji block
    )
}

/// Drawn in two terminal cells because of the script it belongs to, not because it is an
/// emoji. CJK, Hangul syllables, fullwidth forms.
fn is_east_asian_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F                                // Hangul jamo, initial
        | 0x2E80..=0x303E | 0x3041..=0x33FF            // CJK radicals through compat
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF            // CJK ideographs
        | 0xA000..=0xA4CF                              // Yi
        | 0xAC00..=0xD7A3                              // Hangul syllables
        | 0xF900..=0xFAFF                              // CJK compatibility ideographs
        | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6            // fullwidth forms
        | 0x20000..=0x3FFFD                            // CJK extensions B onward
    )
}

fn is_hangul_l(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115F | 0xA960..=0xA97C)
}
fn is_hangul_v(c: char) -> bool {
    matches!(c as u32, 0x1160..=0x11A7 | 0xD7B0..=0xD7C6)
}
fn is_hangul_t(c: char) -> bool {
    matches!(c as u32, 0x11A8..=0x11FF | 0xD7CB..=0xD7FB)
}
/// A precomposed Hangul syllable with no trailing consonant.
fn is_hangul_lv(c: char) -> bool {
    let u = c as u32;
    (0xAC00..=0xD7A3).contains(&u) && (u - 0xAC00).is_multiple_of(28)
}
/// A precomposed Hangul syllable that already has its trailing consonant.
fn is_hangul_lvt(c: char) -> bool {
    let u = c as u32;
    (0xAC00..=0xD7A3).contains(&u) && !(u - 0xAC00).is_multiple_of(28)
}

fn is_control(c: char) -> bool {
    // Everything the terminal treats as a command rather than a mark on the page. The
    // joiner and the marks in that block are Extend, not Control.
    let u = c as u32;
    (u < 0x20 && c != '\r' && c != '\n')
        || u == 0x7F
        || (0x80..=0x9F).contains(&u)
        || matches!(u, 0x2028 | 0x2029)
}

/// What the walk needs to remember about what it has already passed.
#[derive(Default)]
struct Run {
    /// An odd number of regional indicators so far, so the next one completes a flag.
    flag_half_open: bool,
    /// A pictograph, possibly followed by marks, so a joiner here can attach the next one.
    joinable: bool,
}

impl Run {
    fn observe(&mut self, c: char) {
        self.flag_half_open = is_regional_indicator(c) && !self.flag_half_open;
        if is_pictographic(c) {
            self.joinable = true;
        } else if !is_extend(c) && c != ZWJ {
            self.joinable = false;
        }
    }
}

/// Whether a cluster boundary falls between these two scalars.
fn breaks_between(prev: char, next: char, run: &Run) -> bool {
    // A carriage return and its line feed are one thing; every other control character
    // stands alone.
    if prev == '\r' && next == '\n' {
        return false;
    }
    if is_control(prev) || prev == '\r' || prev == '\n' {
        return true;
    }
    if is_control(next) || next == '\r' || next == '\n' {
        return true;
    }

    // Hangul assembles from parts.
    if is_hangul_l(prev)
        && (is_hangul_l(next) || is_hangul_v(next) || is_hangul_lv(next) || is_hangul_lvt(next))
    {
        return false;
    }
    if (is_hangul_lv(prev) || is_hangul_v(prev)) && (is_hangul_v(next) || is_hangul_t(next)) {
        return false;
    }
    if (is_hangul_lvt(prev) || is_hangul_t(prev)) && is_hangul_t(next) {
        return false;
    }

    // Marks and joiners never start anything of their own.
    if is_extend(next) || next == ZWJ {
        return false;
    }

    // A joiner binds one pictograph to the next, which is the whole of `🧑‍🧑‍🧒‍🧒`.
    if prev == ZWJ && run.joinable && is_pictographic(next) {
        return false;
    }

    // Regional indicators pair off, so a third one starts a second flag.
    if is_regional_indicator(prev) && is_regional_indicator(next) && run.flag_half_open {
        return false;
    }

    true
}

/// The grapheme clusters of a string, in order.
pub struct Graphemes<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.rest.is_empty() {
            return None;
        }
        let mut chars = self.rest.char_indices();
        let (_, first) = chars.next()?;
        let mut run = Run::default();
        run.observe(first);
        let mut prev = first;
        let mut end = self.rest.len();

        for (offset, c) in chars {
            if breaks_between(prev, c, &run) {
                end = offset;
                break;
            }
            run.observe(c);
            prev = c;
        }

        let (cluster, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some(cluster)
    }
}

/// Walk a string one reader's-character at a time.
pub fn graphemes(s: &str) -> Graphemes<'_> {
    Graphemes { rest: s }
}

/// How many characters a reader would say this is.
pub fn count(s: &str) -> usize {
    graphemes(s).count()
}

/// How many terminal cells one cluster draws in.
///
/// Zero for a lone mark, two for anything emoji or East Asian, one otherwise.
pub fn cluster_width(cluster: &str) -> usize {
    let mut chars = cluster.chars();
    let Some(first) = chars.next() else {
        return 0;
    };

    // A variation selector can ask for emoji presentation of something that would
    // otherwise be drawn as text, and that changes its width.
    if cluster.chars().any(|c| c == '\u{FE0F}') {
        return 2;
    }
    if is_regional_indicator(first) {
        return 2;
    }
    // The modern emoji blocks are drawn wide; the older symbol ranges are not, unless
    // asked to be, which the check above already covers.
    if matches!(first as u32, 0x1F000..=0x1FAFF) {
        return 2;
    }
    if is_east_asian_wide(first) {
        return 2;
    }
    if is_control(first) || is_extend(first) {
        return 0;
    }
    1
}

/// How many terminal cells the whole string draws in.
///
/// This is what a caret has to be aligned by. It is **not** [`count`], and the difference
/// is exactly why a caret under an emoji lands in the wrong place when someone conflates
/// the two.
pub fn width(s: &str) -> usize {
    graphemes(s).map(cluster_width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name from the error-message design: four people joined into one character.
    const FAMILY: &str = "🧑‍🧑‍🧒‍🧒";

    #[test]
    fn the_family_is_one_character() {
        assert_eq!(FAMILY.chars().count(), 7, "seven scalars");
        assert_eq!(FAMILY.len(), 25, "twenty-five bytes");
        assert_eq!(count(FAMILY), 1, "and one character");
        assert_eq!(graphemes(FAMILY).next(), Some(FAMILY));
    }

    #[test]
    fn a_letter_is_one_character_too() {
        assert_eq!(count("c"), 1);
        assert_eq!(count("hello"), 5);
        assert_eq!(count(""), 0);
    }

    #[test]
    fn ascii_splits_one_byte_at_a_time() {
        let g: Vec<&str> = graphemes("abc").collect();
        assert_eq!(g, ["a", "b", "c"]);
    }

    #[test]
    fn combining_marks_join_what_they_follow() {
        // `e` + combining acute is one character, and so is the precomposed form.
        assert_eq!(count("e\u{0301}"), 1);
        assert_eq!(count("é"), 1);
        assert_eq!(count("cafe\u{0301}"), 4);
        // Several marks on one base still make one.
        assert_eq!(count("a\u{0300}\u{0301}\u{0302}"), 1);
    }

    #[test]
    fn flags_pair_off() {
        let gb = "\u{1F1EC}\u{1F1E7}"; // regional indicators G, B
        assert_eq!(count(gb), 1);
        // Two flags are two characters, not one run of four indicators.
        assert_eq!(count(&format!("{gb}{gb}")), 2);
        // An odd indicator left over stands alone.
        assert_eq!(count(&format!("{gb}\u{1F1EC}")), 2);
    }

    #[test]
    fn skin_tones_and_variation_selectors_attach() {
        assert_eq!(count("\u{1F44D}\u{1F3FF}"), 1, "thumbs up with a skin tone");
        assert_eq!(count("\u{2764}\u{FE0F}"), 1, "heart asking to be an emoji");
    }

    #[test]
    fn joined_emoji_stay_together() {
        assert_eq!(count("\u{1F468}\u{200D}\u{1F4BB}"), 1, "one person, one laptop");
        assert_eq!(count(FAMILY), 1);
        // Two families in a row are two characters, not one long chain.
        assert_eq!(count(&format!("{FAMILY}{FAMILY}")), 2);
    }

    #[test]
    fn a_dangling_joiner_does_not_swallow_a_letter() {
        // ZWJ then something that is not a pictograph: the joiner clings to what precedes
        // it, and the letter starts fresh.
        assert_eq!(count("\u{1F9D1}\u{200D}a"), 2);
    }

    #[test]
    fn newlines_are_their_own_character_and_crlf_is_one() {
        assert_eq!(count("\r\n"), 1);
        assert_eq!(count("a\r\nb"), 3);
        assert_eq!(count("a\nb"), 3);
        assert_eq!(count("\n\n"), 2);
    }

    #[test]
    fn hangul_assembles() {
        // Initial, medial and final jamo make one syllable.
        assert_eq!(count("\u{1100}\u{1161}\u{11A8}"), 1);
        // And a precomposed syllable is already one.
        assert_eq!(count("한"), 1);
        assert_eq!(count("한글"), 2);
    }

    #[test]
    fn width_is_not_the_same_measurement_as_count() {
        // The whole reason both exist.
        assert_eq!(count(FAMILY), 1);
        assert_eq!(width(FAMILY), 2);

        assert_eq!(count("c"), 1);
        assert_eq!(width("c"), 1);

        assert_eq!(count("한"), 1);
        assert_eq!(width("한"), 2);

        // A combining mark takes no room of its own.
        assert_eq!(width("e\u{0301}"), 1);
    }

    #[test]
    fn a_caret_lands_under_the_right_thing() {
        // This is the case the renderer will get wrong if it counts characters: the caret
        // has to be pushed by two cells for the emoji, not one.
        let line = "var.local.b16 ['🧑‍🧑‍🧒‍🧒'] = [|1|];";
        let upto = &line[..line.find("] =").unwrap()];
        assert_eq!(count(upto), 18, "eighteen characters before the bracket");
        assert_eq!(width(upto), 19, "but nineteen cells, because the emoji is wide");
    }

    #[test]
    fn every_cluster_is_a_slice_of_the_original() {
        let s = format!("a{FAMILY}b\u{1F1EC}\u{1F1E7}c한e\u{0301}");
        let joined: String = graphemes(&s).collect();
        assert_eq!(joined, s, "nothing lost and nothing added");
        assert_eq!(count(&s), 7);
    }
}

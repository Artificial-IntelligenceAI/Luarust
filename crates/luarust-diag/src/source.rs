//! A source file, and where things are inside it.
//!
//! Everything downstream refers to a place in the source by a byte range — a [`Span`] —
//! because that is the only measurement that survives being handed around. Turning one
//! back into something a person can read is this module's job, and it produces all three
//! numbers the error format needs.

use crate::grapheme;
use std::path::{Path, PathBuf};

/// A byte range in one source file. Half-open: `start` is included, `end` is not.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end, "a span cannot end before it starts");
        Self { start, end }
    }

    /// A span of no width, for pointing between two things rather than at one.
    pub fn at(offset: usize) -> Self {
        Self { start: offset, end: offset }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The smallest span covering both.
    pub fn to(self, other: Span) -> Span {
        Span { start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

/// Where a byte offset is, said three ways.
///
/// All three are 1-based, because that is what every editor and every reader expects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Position {
    /// Which line, counting from one.
    pub line: usize,
    /// Which character on it, counting the way a reader counts — `🧑‍🧑‍🧒‍🧒` is one.
    pub column: usize,
    /// Which byte on it. This is the one `file:line:column` carries, because it is the
    /// one an editor or a `grep` will agree with.
    pub byte_column: usize,
}

/// One source file, held with an index of where its lines begin.
pub struct SourceFile {
    path: PathBuf,
    text: String,
    /// Byte offset of the first character of each line.
    line_starts: Vec<usize>,
    /// How long the text was, for a file whose text was not kept. `None` when it was.
    absent_len: Option<usize>,
}

impl SourceFile {
    /// A file whose text was not kept, but whose shape was.
    ///
    /// A chunk built without its source still knows where every line began, so a fault in
    /// it reports the same line and column it always did. What it cannot do is show the
    /// line, and saying so is better than showing a blank one.
    pub fn without_text(path: impl Into<PathBuf>, line_starts: Vec<usize>, len: usize) -> Self {
        Self { path: path.into(), text: String::new(), line_starts, absent_len: Some(len) }
    }

    /// Whether the text itself is here, as opposed to only the shape of it.
    pub fn has_text(&self) -> bool {
        self.absent_len.is_none()
    }

    pub fn new(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        Self { path: path.into(), text, line_starts, absent_len: None }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The path as the short `file:line:column` form should show it: relative to where
    /// the compiler was run, when that is shorter, and absolute when it is not.
    pub fn display_path(&self) -> String {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| self.path.strip_prefix(&cwd).ok().map(Path::to_path_buf))
            .unwrap_or_else(|| self.path.clone())
            .display()
            .to_string()
    }

    /// Which line an offset falls on, counting from one. An offset past the end belongs
    /// to the last line.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(index) => index + 1,
            Err(index) => index, // the insertion point is one past the line it belongs to
        }
    }

    /// One line's text, without its line ending. Lines count from one.
    pub fn line_text(&self, line: usize) -> &str {
        if line == 0 || line > self.line_starts.len() {
            return "";
        }
        let start = self.line_starts[line - 1];
        let end = self
            .line_starts
            .get(line)
            .map(|next| next - 1) // step back over the '\n'
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches('\r')
    }

    /// Where an offset is, in all three measurements at once.
    pub fn position(&self, offset: usize) -> Position {
        let offset = offset.min(self.len());
        let line = self.line_of(offset);
        let line_start = self.line_starts[line - 1];

        // Without the text, the line and the byte column are still exact and the grapheme
        // column cannot be: counting graphemes means having the characters. It answers the
        // byte column for both rather than inventing a number.
        let Some(prefix) = self.text.get(line_start..offset) else {
            let bytes = offset - line_start;
            return Position { line, column: bytes + 1, byte_column: bytes + 1 };
        };

        Position {
            line,
            column: grapheme::count(prefix) + 1,
            byte_column: prefix.len() + 1,
        }
    }

    /// How long the file was, whether or not its text was kept.
    fn len(&self) -> usize {
        self.absent_len.unwrap_or(self.text.len())
    }

    /// The `file:line:column` form, carrying the byte column.
    pub fn short_location(&self, offset: usize) -> String {
        let at = self.position(offset);
        format!("{}:{}:{}", self.display_path(), at.line, at.byte_column)
    }

    /// The source a span covers.
    pub fn slice(&self, span: Span) -> &str {
        let start = span.start.min(self.text.len());
        let end = span.end.min(self.text.len());
        &self.text[start..end]
    }

    /// How a caret under this span should be laid out: how many terminal cells to skip,
    /// and how many to underline.
    ///
    /// Cells, not characters — an emoji draws two of them where a letter draws one, so a
    /// caret positioned by counting characters lands to the left of what it means.
    pub fn caret_layout(&self, span: Span) -> (usize, usize) {
        let line = self.line_of(span.start);
        let line_start = self.line_starts[line - 1];
        let start = span.start.min(self.text.len());
        let end = span.end.min(self.text.len()).max(start);
        let indent = grapheme::width(&self.text[line_start..start]);
        // An empty span still gets one cell, so there is something to see.
        let under = grapheme::width(&self.text[start..end]).max(1);
        (indent, under)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAMILY: &str = "🧑‍🧑‍🧒‍🧒";

    fn file(text: &str) -> SourceFile {
        SourceFile::new("src/main.lr", text)
    }

    #[test]
    fn a_position_counts_from_one() {
        let f = file("abc\ndef\n");
        let at = f.position(0);
        assert_eq!((at.line, at.column, at.byte_column), (1, 1, 1));
    }

    #[test]
    fn lines_are_found_by_offset() {
        let f = file("abc\ndef\nghi");
        assert_eq!(f.line_of(0), 1);
        assert_eq!(f.line_of(2), 1);
        assert_eq!(f.line_of(3), 1, "the newline belongs to the line it ends");
        assert_eq!(f.line_of(4), 2);
        assert_eq!(f.line_of(8), 3);
        assert_eq!(f.line_of(999), 3, "past the end is the last line");
        assert_eq!(f.line_count(), 3);
    }

    #[test]
    fn line_text_excludes_the_ending() {
        let f = file("abc\ndef\nghi");
        assert_eq!(f.line_text(1), "abc");
        assert_eq!(f.line_text(2), "def");
        assert_eq!(f.line_text(3), "ghi");
        assert_eq!(f.line_text(0), "");
        assert_eq!(f.line_text(4), "");
    }

    #[test]
    fn crlf_is_not_left_dangling_on_the_line() {
        let f = file("abc\r\ndef");
        assert_eq!(f.line_text(1), "abc");
        assert_eq!(f.line_text(2), "def");
        assert_eq!(f.line_of(5), 2);
    }

    #[test]
    fn a_trailing_newline_opens_an_empty_last_line() {
        let f = file("abc\n");
        assert_eq!(f.line_count(), 2);
        assert_eq!(f.line_text(2), "");
        let at = f.position(4);
        assert_eq!((at.line, at.column), (2, 1));
    }

    #[test]
    fn the_two_columns_agree_while_the_text_is_ascii() {
        let f = file("var.local.b16 ['x'] = ['1'];");
        for offset in 0..f.text().len() {
            let at = f.position(offset);
            assert_eq!(at.column, at.byte_column, "at byte {offset}");
            assert_eq!(at.column, offset + 1);
        }
    }

    #[test]
    fn the_two_columns_part_company_at_an_emoji() {
        // This is the case the whole three-measurement design exists for.
        let line = format!("var.local.b16 ['{FAMILY}'] = ['1'];");
        let f = file(&line);
        let bracket = line.find("] =").unwrap();

        let at = f.position(bracket);
        assert_eq!(at.line, 1);
        assert_eq!(at.column, 19, "nineteen characters in, as a reader counts");
        assert_eq!(at.byte_column, 43, "but forty-three bytes in");
        assert_eq!(f.short_location(bracket), "src/main.lr:1:43");
    }

    #[test]
    fn a_caret_is_laid_out_in_cells_not_characters() {
        let line = format!("var.local.b16 ['{FAMILY}'] = ['1'];");
        let f = file(&line);
        let name_start = line.find(FAMILY).unwrap();
        let span = Span::new(name_start, name_start + FAMILY.len());

        let (indent, under) = f.caret_layout(span);
        assert_eq!(indent, 16, "sixteen cells of `var.local.b16 ['`");
        assert_eq!(under, 2, "and the emoji draws two");

        // The position's column, by contrast, counts it as one character.
        assert_eq!(f.position(name_start).column, 17);
    }

    #[test]
    fn an_empty_span_still_gets_a_caret() {
        let f = file("abc");
        let (indent, under) = f.caret_layout(Span::at(2));
        assert_eq!((indent, under), (2, 1));
    }

    #[test]
    fn spans_slice_and_join() {
        let f = file("var.local.str ['name'];");
        let span = Span::new(15, 21);
        assert_eq!(f.slice(span), "'name'");
        assert_eq!(span.len(), 6);
        assert!(!span.is_empty());
        assert!(Span::at(4).is_empty());
        assert_eq!(Span::new(2, 5).to(Span::new(9, 12)), Span::new(2, 12));
    }

    #[test]
    fn offsets_past_the_end_do_not_panic() {
        let f = file("abc");
        let at = f.position(999);
        assert_eq!((at.line, at.column, at.byte_column), (1, 4, 4));
        assert_eq!(f.slice(Span::new(1, 999)), "bc");
        assert_eq!(f.caret_layout(Span::new(999, 1000)), (3, 1));
    }

    #[test]
    fn positions_on_a_later_line_restart_their_columns() {
        let f = file("var.local.b16 ['x'] = ['1'];\nprint['x' \\n];");
        let print_at = f.text().find("print").unwrap();
        let at = f.position(print_at);
        assert_eq!((at.line, at.column, at.byte_column), (2, 1, 1));
    }
}

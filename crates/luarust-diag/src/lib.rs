//! Where a thing is in a source file, and how to say so.
//!
//! Luarust reports a position three different ways at once, because a position is three
//! different numbers and only one of them is the one a person means:
//!
//! - the **column a reader is given** counts graphemes, so `🧑‍🧑‍🧒‍🧒` is one character
//!   exactly as `c` is;
//! - the **column in `file:line:column`** counts bytes, because that form exists to be
//!   pasted into an editor or a `grep`, and bytes are what those understand;
//! - and the **caret** is placed by terminal cells, because that emoji draws two cells
//!   wide where a letter draws one.
//!
//! Get any of the three wrong and the error still looks plausible, which is why they are
//! separated here rather than left for whoever writes the renderer to conflate.

pub mod grapheme;
pub mod source;

pub use source::{Position, SourceFile, Span};

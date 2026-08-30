//! What a piece of Luarust source turns into.

use luarust_diag::Span;

/// One token, and where it came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Token {
    pub kind: Kind,
    pub span: Span,
}

/// The kinds of thing a Luarust program is made of.
///
/// Bare words are **not** sorted into keywords here. `local`, `mut`, `b16`, `range`, `mod`
/// and `x` are all just [`Kind::Word`], and the parser decides what each means where it
/// stands — `error` is a behaviour in `defaults.no-visibility-stated.error` and would be
/// nothing of the kind elsewhere. Sorting them out this early would mean reserving words
/// the language never needed to reserve, and it can afford not to: a name is always
/// quoted, so a bare word can never be one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `[` — opens a list of names, of values, or of things to print.
    OpenList,
    /// `]`
    CloseList,
    /// `{` — opens a block. The word in front of it says which kind.
    OpenBlock,
    /// `}`
    CloseBlock,
    /// `(` — grouping, inside a math block.
    OpenGroup,
    /// `)`
    CloseGroup,

    /// `;` — ends a statement that finished on a value.
    Semicolon,
    /// `,` — between the items of a list.
    Comma,
    /// `.` — between the parts of a chain.
    Dot,
    /// `=`
    Equals,

    Plus,
    Minus,
    /// `*` or `x`, though the word spelling arrives as a [`Kind::Word`].
    Star,
    /// `**`
    StarStar,
    /// `/` or `÷`
    Slash,
    /// `%` — percent, written after a number. Never remainder; that is the word `mod`.
    Percent,

    /// A bare word: a keyword, a chain part, a type, or a word-spelled operator.
    Word,
    /// A bare number, which only appears inside a math block.
    Number,
    /// `'…'` — a name, or a literal where a value is expected.
    Name,
    /// `"…"` — text.
    Text,
    /// `\n` and friends, written outside the quotes.
    Escape,

    /// The end of the file. Always the last token, so a parser can look ahead safely.
    End,
}

impl Kind {
    /// What to call this in an error message.
    pub fn describe(self) -> &'static str {
        match self {
            Kind::OpenList => "`[`",
            Kind::CloseList => "`]`",
            Kind::OpenBlock => "`{`",
            Kind::CloseBlock => "`}`",
            Kind::OpenGroup => "`(`",
            Kind::CloseGroup => "`)`",
            Kind::Semicolon => "`;`",
            Kind::Comma => "`,`",
            Kind::Dot => "`.`",
            Kind::Equals => "`=`",
            Kind::Plus => "`+`",
            Kind::Minus => "`-`",
            Kind::Star => "`*`",
            Kind::StarStar => "`**`",
            Kind::Slash => "`/`",
            Kind::Percent => "`%`",
            Kind::Word => "a word",
            Kind::Number => "a number",
            Kind::Name => "a name",
            Kind::Text => "text",
            Kind::Escape => "an escape",
            Kind::End => "the end of the file",
        }
    }
}

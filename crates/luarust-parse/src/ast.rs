//! The shape of a Luarust program once it has been read.
//!
//! Every node carries the span it came from, because every one of them may end up named
//! in an error and an error that cannot point at anything is not worth much.

use luarust_diag::Span;

// These three say nothing about syntax -- they are the types and operators a program
// still needs while it is running, long after the parser is gone -- so they live in
// `luarust-core` and are only named from here.
pub use luarust_core::{BinOp, CmpOp, LogicOp, Ty};

/// A name, as written between its quotes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ident {
    pub text: String,
    /// The whole `'…'`, quotes included, so a caret underlines what was written.
    pub span: Span,
}

/// Who can see a variable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Visibility {
    /// The block it is written in, and no further.
    Local,
    /// The whole program.
    Global,
    /// Exported, so importers see it too.
    Public,
    /// Nobody, anywhere. What a declaration means when it says nothing.
    Restricted,
}

impl Visibility {
    pub fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "local" => Visibility::Local,
            "global" => Visibility::Global,
            "public" => Visibility::Public,
            "restricted" => Visibility::Restricted,
            _ => return None,
        })
    }

    pub fn word(self) -> &'static str {
        match self {
            Visibility::Local => "local",
            Visibility::Global => "global",
            Visibility::Public => "public",
            Visibility::Restricted => "restricted",
        }
    }
}

/// How long a loop's counter lives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lifetime {
    /// Gone at the closing brace.
    Temp,
    /// Still there afterwards, holding the last value it took.
    Perm,
}

/// One declared variable, with every part of its chain resolved.
#[derive(Clone, Debug)]
pub struct Binding {
    pub span: Span,
    pub name: Ident,
    pub visibility: Visibility,
    /// Where the visibility was written, if it was.
    pub visibility_span: Option<Span>,
    pub mutable: bool,
    pub mutable_span: Option<Span>,
    pub ty: Ty,
    pub ty_span: Span,
}

/// Anything that produces a value.
#[derive(Clone, Debug)]
pub enum Expr {
    /// `'…'` where a value is expected: read as whatever type the annotation asks for.
    Literal { text: String, span: Span },
    /// `'…'` where a value is being read: a variable.
    Name(Ident),
    /// `b64 '1.5'` — a literal that says what it is, for the places nothing else does.
    TypedLiteral { ty: Ty, text: String, span: Span },
    /// A bare number, which only appears inside a math block.
    Number { text: String, span: Span },
    /// `time.now`
    TimeNow { span: Span },
    /// `20%` — the number before it, divided by a hundred.
    Percent { inner: Box<Expr>, span: Span },
    Unary { op: BinOp, operand: Box<Expr>, span: Span },
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `math { … }`. Kept as a node because it is the boundary where bare numbers and
    /// operators become legal.
    Math { inner: Box<Expr>, span: Span },
    /// `a < b`, `a > b`, `a = b`. Answers `bool` whatever its two sides were.
    /// `and` or `or`. Both sides are conditions, and so is the answer.
    Logic { op: LogicOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `not`, which turns a condition around.
    Not { operand: Box<Expr>, span: Span },
    Compare { op: CmpOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Literal { span, .. }
            | Expr::Number { span, .. }
            | Expr::TimeNow { span }
            | Expr::Percent { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Math { span, .. }
            | Expr::TypedLiteral { span, .. }
            | Expr::Compare { span, .. }
            | Expr::Logic { span, .. }
            | Expr::Not { span, .. } => *span,
            Expr::Name(ident) => ident.span,
        }
    }
}

/// One item in a print list.
#[derive(Clone, Debug)]
pub enum PrintItem {
    /// `"…"`, with its escapes already resolved.
    Text { value: String, span: Span },
    /// A bare `\n` and friends.
    Escape { value: char, span: Span },
    /// A variable, or a math block.
    Value(Expr),
}

#[derive(Clone, Debug)]
pub struct Var {
    pub span: Span,
    pub bindings: Vec<Binding>,
    pub values: Vec<Expr>,
    pub names_span: Span,
    pub values_span: Span,
}

#[derive(Clone, Debug)]
pub struct Set {
    pub span: Span,
    pub targets: Vec<Ident>,
    pub values: Vec<Expr>,
    pub names_span: Span,
    pub values_span: Span,
}

#[derive(Clone, Debug)]
pub struct Handback {
    pub span: Span,
    pub source: Ident,
    pub target: Ident,
}

#[derive(Clone, Debug)]
pub struct Print {
    pub span: Span,
    pub items: Vec<PrintItem>,
}

#[derive(Clone, Debug)]
pub struct Loop {
    pub span: Span,
    pub lifetime: Lifetime,
    pub lifetime_span: Span,
    pub counter: Ident,
    pub ty: Ty,
    pub ty_span: Span,
    pub from: Expr,
    pub to: Expr,
    pub body: Vec<Stmt>,
}

/// `defaults.something.behaviour;`
#[derive(Clone, Debug)]
pub struct Defaults {
    pub span: Span,
    pub setting: String,
    pub setting_span: Span,
    pub behaviour: String,
    pub behaviour_span: Span,
}

/// One `if` or `else-if`: something to ask, and what to do when the answer is yes.
#[derive(Clone, Debug)]
pub struct Arm {
    pub span: Span,
    pub condition: Expr,
    pub body: Vec<Stmt>,
}

/// `if [ … ] { … } else-if [ … ] { … } else { … }`
///
/// The `if` and every `else-if` are the same shape, so they are one list. `else` has no
/// condition, which is the only thing that makes it different.
#[derive(Clone, Debug)]
pub struct If {
    pub span: Span,
    pub arms: Vec<Arm>,
    pub otherwise: Option<Vec<Stmt>>,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Var(Var),
    Set(Set),
    Handback(Handback),
    Print(Print),
    Loop(Loop),
    If(If),
    Defaults(Defaults),
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Var(s) => s.span,
            Stmt::Set(s) => s.span,
            Stmt::Handback(s) => s.span,
            Stmt::Print(s) => s.span,
            Stmt::Loop(s) => s.span,
            Stmt::If(s) => s.span,
            Stmt::Defaults(s) => s.span,
        }
    }
}

/// A whole file.
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

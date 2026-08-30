//! What the checker hands to whatever runs the program.
//!
//! Everything is already decided here. A name has become a slot, a written literal has
//! become a value of a known type, and every operation knows what it is operating on — so
//! an interpreter, a bytecode compiler and a JIT can each take this and none of them has
//! to work out the same things again, or risk working them out differently.

use crate::value::{Overflow, Value};
use luarust_diag::Span;
use luarust_parse::ast::{BinOp, Ty};

/// A whole checked program.
#[derive(Clone, Debug)]
pub struct Checked {
    pub stmts: Vec<Stmt>,
    /// How many variables the program needs room for.
    pub slots: usize,
    /// What to do when a whole number will not fit.
    pub overflow: Overflow,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// Put a value in a slot. Declaring and changing are the same act once names are gone.
    Store { slot: usize, value: Expr, span: Span },
    Print { items: Vec<Item>, span: Span },
    /// Count from `from` to `to`, inclusive, storing each value into `slot`.
    Loop {
        slot: usize,
        ty: Ty,
        from: Expr,
        to: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub enum Item {
    /// Text as written, escapes already resolved.
    Text(String),
    /// Something to work out and then stringify.
    Value(Expr),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Const(Value),
    Load { slot: usize, ty: Ty, span: Span },
    /// The monotonic clock, in seconds.
    TimeNow { ty: Ty, span: Span },
    Binary { op: BinOp, ty: Ty, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    Neg { ty: Ty, operand: Box<Expr>, span: Span },
}

impl Expr {
    pub fn ty(&self) -> Ty {
        match self {
            Expr::Const(value) => value.ty(),
            Expr::Load { ty, .. }
            | Expr::TimeNow { ty, .. }
            | Expr::Binary { ty, .. }
            | Expr::Neg { ty, .. } => *ty,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Expr::Const(_) => Span::default(),
            Expr::Load { span, .. }
            | Expr::TimeNow { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Neg { span, .. } => *span,
        }
    }
}

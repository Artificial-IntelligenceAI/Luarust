//! What the checker hands to whatever runs the program.
//!
//! Everything is already decided here. A name has become a slot, a written literal has
//! become a value of a known type, and every operation knows what it is operating on — so
//! an interpreter, a bytecode compiler and a JIT can each take this and none of them has
//! to work out the same things again, or risk working them out differently.

use crate::value::{Engine, Floats, Overflow, Value};
use luarust_core::heap::Collect;
use luarust_diag::Span;
use luarust_parse::ast::{BinOp, CmpOp, LogicOp, Ty};

/// A whole checked program.
#[derive(Clone, Debug)]
pub struct Checked {
    pub stmts: Vec<Stmt>,
    /// Every function in the program, in the order they were declared. A call carries
    /// the index rather than the name -- names were the front end's business.
    pub funcs: Vec<Function>,
    /// How many variables the program needs room for.
    pub slots: usize,
    /// What to do when a whole number will not fit.
    pub overflow: Overflow,
    /// What to do about arrays nothing can reach, and how much of a float to write out.
    /// Settled by the project and carried from here into the chunk, so that a program
    /// keeps its own answers wherever it is run.
    pub collect: Collect,
    pub floats: Floats,
    /// Which engine the project asked to run this with.
    pub engine: Engine,
}

/// One function, with its own slots. The first `params` of them are its parameters, in
/// order, which is what lets a call put its arguments straight where they belong.
#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub params: Vec<Ty>,
    /// `None` when it answers nothing.
    pub returns: Option<Ty>,
    pub slots: usize,
    pub body: Vec<Stmt>,
    pub span: Span,
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
    /// Ask, in order, and run the first body whose condition held. `otherwise` is the
    /// `else`, and is empty when there was none.
    If { arms: Vec<Arm>, otherwise: Vec<Stmt>, span: Span },
    /// Put a value in one element of an array.
    StoreAt { array: Expr, at: Vec<Expr>, value: Expr, span: Span },
    /// Leave the function, with a value when it has one to give.
    Return { value: Option<Expr>, span: Span },
    /// Call something for what it does. Whatever it answers, if anything, is dropped.
    Call { func: usize, args: Vec<Expr>, span: Span },
    /// Run the body while the condition holds, asked again before every pass. `counter`
    /// is the slot counting passes, when the loop asked for one.
    While {
        counter: Option<(usize, Ty)>,
        condition: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    /// Leave the innermost loop. `break when reached` is not here: the checker turns it
    /// into an `if` around one of these, so nothing downstream has two things to learn.
    Break { span: Span },
}

/// One condition and what to do about it.
#[derive(Clone, Debug)]
pub struct Arm {
    pub condition: Expr,
    pub body: Vec<Stmt>,
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
    /// Answers `bool`. `operands` is what the two sides are, which is what decides how
    /// they get compared.
    Compare { op: CmpOp, operands: Ty, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `and` / `or`. The right side is only worked out if the left did not settle it,
    /// which is what lets a condition guard the one after it.
    Logic { op: LogicOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    Not { operand: Box<Expr>, span: Span },
    /// `func` indexes [`Checked::funcs`]; `ty` is what that function answers, which a
    /// call in a value position must have.
    Call { func: usize, ty: Ty, args: Vec<Expr>, span: Span },
    /// A new array holding these, in order. New every time it is reached, because two
    /// passes of a loop must not be writing into one array.
    NewArray { ty: Ty, items: Vec<Expr>, span: Span },
    /// A new array of `length` elements, every one of them `value`.
    Filled { ty: Ty, length: Box<Expr>, value: Box<Expr>, span: Span },
    /// One element. `ty` is the element's type; `at` has one index per dimension.
    At { array: Box<Expr>, at: Vec<Expr>, ty: Ty, span: Span },
    /// How many elements an array holds, as whatever type is expecting the answer.
    Count { array: Box<Expr>, ty: Ty, span: Span },
}

impl Expr {
    pub fn ty(&self) -> Ty {
        match self {
            Expr::Const(value) => value.ty(),
            Expr::Load { ty, .. }
            | Expr::TimeNow { ty, .. }
            | Expr::Binary { ty, .. }
            | Expr::Neg { ty, .. } => *ty,
            Expr::Compare { .. } | Expr::Logic { .. } | Expr::Not { .. } => Ty::Bool,
            Expr::Call { ty, .. }
            | Expr::NewArray { ty, .. }
            | Expr::Filled { ty, .. }
            | Expr::At { ty, .. }
            | Expr::Count { ty, .. } => *ty,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Expr::Const(_) => Span::default(),
            Expr::Load { span, .. }
            | Expr::TimeNow { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Neg { span, .. }
            | Expr::Compare { span, .. }
            | Expr::Logic { span, .. }
            | Expr::Call { span, .. }
            | Expr::NewArray { span, .. }
            | Expr::Filled { span, .. }
            | Expr::At { span, .. }
            | Expr::Count { span, .. }
            | Expr::Not { span, .. } => *span,
        }
    }
}

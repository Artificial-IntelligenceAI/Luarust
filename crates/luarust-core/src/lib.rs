//! What a running program needs, and nothing else.
//!
//! A Luarust program that has already been compiled has no use for a lexer, a parser or
//! a checker: those did their work and are over. What it still needs is the meaning of a
//! value and the meaning of an operator, which is what lives here.
//!
//! Keeping them in a leaf crate is not tidiness. It is the difference between a delivered
//! program carrying a runtime and a delivered program carrying the whole toolchain, and
//! the rule for this language is that nothing ships that the program does not use.

pub mod ty;
pub mod value;

pub use ty::{BinOp, CmpOp, Ty};

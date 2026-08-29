//! Luarust's numeric tower.
//!
//! Luarust has no integer type. Every number in the language is one of
//!
//! ```text
//! b16  b32  b64  b128  b256    IEEE 754 binary   (binary16 … binary256)
//! d32  d64  d128               IEEE 754 decimal  (decimal32/64/128)
//! er                           an unbounded rational — no rounding, no overflow
//! ```
//!
//! and exactly two of those, `b32` and `b64`, are formats the hardware knows about.
//! The rest are arithmetic this crate performs itself, bit by bit, to the rounding
//! the standard requires. That is the reason this crate exists and the reason it is
//! the first thing in the workspace: the compiler, the bytecode VM and the JIT all
//! need the same answers, and they get them from here.
//!
//! The crate has no dependencies, including no `libm` and no bignum crate.

pub mod binary;
pub mod uint;

pub use binary::{Class, Format, Round, Unpacked};
pub use uint::Uint;

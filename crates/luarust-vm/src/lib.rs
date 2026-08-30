//! Luarust's bytecode, and the machine that runs it.
//!
//! This is the second of the three ways to run a Luarust program, and the first one that
//! makes the others worth having: with only a tree-walker there is nothing to disagree
//! with, and with two implementations that must produce the same answers, every
//! disagreement is a bug one of them is hiding.
//!
//! It is also the artifact the README promises. A chunk is what "compile once, run
//! anywhere" compiles to.
//!
//! Nothing here reimplements arithmetic. Every number still comes from `luarust-num` by
//! way of `luarust-check`, so the VM and the interpreter agree because they are running
//! the same code, not because two people were careful.

pub mod chunk;
/// Turning a checked program into a chunk -- which a machine that only *runs* chunks has
/// no use for, so it is a feature and not a fact.
#[cfg(feature = "compile")]
pub mod compile;
pub mod serialize;

pub use chunk::{Chunk, Op};
#[cfg(feature = "compile")]
pub use compile::compile;
pub use serialize::{Broken, Loaded, read, write};

use luarust_core::value::{
    Stopped, Value, binary_op, compare, format_of, holds, int_op, negate,
};
use luarust_num::binary::{self, Comparison, Round};
use std::io::Write;
use std::time::Instant;

/// Run a compiled chunk.
pub fn run(chunk: &Chunk, out: &mut impl Write) -> Result<(), Stopped> {
    // A register the checker has proved is written before it is read. The placeholder is
    // never observed by a program that got this far.
    let placeholder = Value::Bool(false);
    let mut registers = vec![placeholder; chunk.registers];
    let started = Instant::now();
    let mut at = 0usize;

    loop {
        let op = chunk.code[at];
        let span = chunk.spans[at];
        at += 1;

        match op {
            Op::Halt => return Ok(()),

            Op::Const { dst, konst } => {
                registers[dst as usize] = chunk.consts[konst as usize].clone();
            }

            Op::Move { dst, src } => {
                registers[dst as usize] = registers[src as usize].clone();
            }

            // Integers go straight to the arithmetic with their raw bits, since the
            // instruction already says what width they are. Everything else goes the long
            // way round, which for a b256 divide is nothing next to the divide.
            Op::Binary { op, ty, dst, lhs, rhs } if ty.is_integer() => {
                let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) =
                    (&registers[lhs as usize], &registers[rhs as usize])
                else {
                    unreachable!("the checker said these are integers")
                };
                let bits = int_op(op, ty, *a, *b, chunk.overflow)
                    .map_err(|fault| Stopped { fault, span })?;
                registers[dst as usize] = Value::Num { ty, bits };
            }

            Op::Binary { op, dst, lhs, rhs, .. } => {
                let value = binary_op(
                    op,
                    &registers[lhs as usize],
                    &registers[rhs as usize],
                    chunk.overflow,
                )
                .map_err(|fault| Stopped { fault, span })?;
                registers[dst as usize] = value;
            }

            Op::Compare { op, dst, lhs, rhs, .. } => {
                let ordering = compare(&registers[lhs as usize], &registers[rhs as usize]);
                registers[dst as usize] = Value::Bool(holds(op, ordering));
            }

            Op::Neg { dst, src } => {
                let value = negate(&registers[src as usize], chunk.overflow)
                    .map_err(|fault| Stopped { fault, span })?;
                registers[dst as usize] = value;
            }

            Op::TimeNow { dst, ty } => {
                let seconds = started.elapsed().as_secs_f64();
                let fmt = format_of(ty).expect("the clock is read as a float");
                let bits = binary::from_decimal::<8>(
                    fmt,
                    Round::TiesToEven,
                    &format!("{seconds:.9}"),
                )
                .expect("nine decimal places is a number");
                registers[dst as usize] = Value::float(ty, bits);
            }

            Op::PrintText { text } => {
                let _ = out.write_all(chunk.texts[text as usize].as_bytes());
                let _ = out.flush();
            }

            Op::PrintValue { src } => {
                let _ = out.write_all(registers[src as usize].to_string().as_bytes());
                let _ = out.flush();
            }

            Op::Not { dst, src } => {
                registers[dst as usize] = Value::Bool(!truth(&registers[src as usize]));
            }

            Op::JumpIfFalse { cond, target } => {
                if !truth(&registers[cond as usize]) {
                    at = target as usize;
                }
            }

            Op::JumpIfTrue { cond, target } => {
                if truth(&registers[cond as usize]) {
                    at = target as usize;
                }
            }

            Op::Jump { target } => at = target as usize,

            Op::JumpIfGreater { lhs, rhs, target } => {
                if compare(&registers[lhs as usize], &registers[rhs as usize])
                    == Comparison::Greater
                {
                    at = target as usize;
                }
            }

            Op::JumpIfEqual { lhs, rhs, target } => {
                if compare(&registers[lhs as usize], &registers[rhs as usize])
                    == Comparison::Equal
                {
                    at = target as usize;
                }
            }
        }
    }
}

/// What a condition answered. The checker refuses anything that is not a `bool` long
/// before a chunk exists, and a chunk read off disk has its own check, so there is
/// nothing to decide here.
fn truth(value: &Value) -> bool {
    match value {
        Value::Bool(answer) => *answer,
        other => unreachable!("a condition checked as `bool` held {other:?}"),
    }
}

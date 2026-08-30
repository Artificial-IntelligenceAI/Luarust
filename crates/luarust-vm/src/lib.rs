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
    DEPTH_LIMIT, Fault, Stopped, Value, binary_op, compare, format_of, holds, int_op, negate,
};
use luarust_num::binary::{self, Comparison, Round};
use std::io::Write;
use std::time::Instant;


/// One call in progress: where it is, what it is holding, and where its answer goes.
struct Frame {
    /// The function being run, or `None` for the top level.
    routine: Option<usize>,
    at: usize,
    registers: Vec<Value>,
    /// The register in the *caller* that receives the answer.
    dst: u16,
}

/// Run a compiled chunk.
pub fn run(chunk: &Chunk, out: &mut impl Write) -> Result<(), Stopped> {
    // A register the checker has proved is written before it is read. The placeholder is
    // never observed by a program that got this far.
    let placeholder = Value::Bool(false);
    let started = Instant::now();

    let mut frames: Vec<Frame> = vec![Frame {
        routine: None,
        at: 0,
        registers: vec![placeholder.clone(); chunk.registers],
        dst: 0,
    }];

    loop {
        let depth = frames.len() - 1;
        let frame = frames.last_mut().expect("a frame is always open");
        let code = match frame.routine {
            None => &chunk.code,
            Some(index) => &chunk.funcs[index].code,
        };
        let spans = match frame.routine {
            None => &chunk.spans,
            Some(index) => &chunk.funcs[index].spans,
        };
        let at = frame.at;
        let op = code[at];
        let span = spans[at];
        frame.at += 1;
        let registers = &mut frame.registers;
        // Where to go next, decided inside the match and applied once it lets go of the
        // frame it is holding.
        let mut jump: Option<usize> = None;

        match op {
            Op::Halt => return Ok(()),

            Op::Call { func, base, argc, dst } => {
                if depth >= DEPTH_LIMIT {
                    return Err(Stopped {
                        fault: Fault {
                            code: "R0011",
                            message: format!("this has called itself {DEPTH_LIMIT} deep."),
                            rule: "a call may only go so deep before the program is stopped",
                            fix: "give the recursion a case that stops, or write it as a loop."
                                .to_string(),
                        },
                        span,
                    });
                }
                let routine = &chunk.funcs[func as usize];
                let mut fresh = vec![placeholder.clone(); routine.registers];
                for n in 0..argc as usize {
                    fresh[n] = registers[base as usize + n].clone();
                }
                frames.push(Frame { routine: Some(func as usize), at: 0, registers: fresh, dst });
                continue;
            }

            Op::Return { src } => {
                let answer = registers[src as usize].clone();
                let finished = frames.pop().expect("a frame is always open");
                let caller = frames.last_mut().expect("something called it");
                caller.registers[finished.dst as usize] = answer;
                continue;
            }

            Op::ReturnNothing => {
                frames.pop();
                continue;
            }

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
                    jump = Some(target as usize);
                }
            }

            Op::JumpIfTrue { cond, target } => {
                if truth(&registers[cond as usize]) {
                    jump = Some(target as usize);
                }
            }

            Op::Jump { target } => jump = Some(target as usize),

            Op::JumpIfGreater { lhs, rhs, target } => {
                if compare(&registers[lhs as usize], &registers[rhs as usize])
                    == Comparison::Greater
                {
                    jump = Some(target as usize);
                }
            }

            Op::JumpIfEqual { lhs, rhs, target } => {
                if compare(&registers[lhs as usize], &registers[rhs as usize])
                    == Comparison::Equal
                {
                    jump = Some(target as usize);
                }
            }
        }

        if let Some(to) = jump {
            frames.last_mut().expect("a frame is always open").at = to;
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

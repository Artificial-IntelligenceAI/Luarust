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
    DEPTH_LIMIT, Fault, Stopped, Value, int_compare, binary_op, compare, format_of, holds, int_op, negate,
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

    // Two loops rather than one. The outer runs once per call, and settles which code is
    // being run and where its registers are; the inner runs once per instruction and has
    // both of those in hand already. Doing it in one loop meant finding the frame, and
    // then matching on which routine it was in, before every single instruction -- work
    // that only changes when a call does.
    'activation: loop {
        let (routine, mut at) = {
            let frame = frames.last().expect("a frame is always open");
            (frame.routine, frame.at)
        };
        let (code, spans) = match routine {
            None => (&chunk.code[..], &chunk.spans[..]),
            Some(index) => (&chunk.funcs[index].code[..], &chunk.funcs[index].spans[..]),
        };
        let depth = frames.len() - 1;

        // What ended the inner loop, decided while the registers are still borrowed and
        // acted on once they are not.
        let step = {
            let registers = &mut frames.last_mut().expect("a frame is always open").registers;
            loop {
                let op = code[at];
                // Where this instruction came from is only ever wanted when something
                // goes wrong, and nothing goes wrong on the overwhelming majority of
                // instructions. Fetching it here cost sixteen bytes a time for the
                // benefit of the path that does not run.
                let here = at;
                at += 1;

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
                        span: spans[here],
                    });
                }
                let mut fresh = vec![placeholder.clone(); chunk.funcs[func as usize].registers];
                for n in 0..argc as usize {
                    fresh[n] = registers[base as usize + n].clone();
                }
                break Step::Called { func: func as usize, fresh, dst };
            }

            Op::Return { src, .. } => break Step::Returned(Some(registers[src as usize].clone())),

            Op::ReturnNothing => break Step::Returned(None),

            Op::Const { dst, konst } => {
                registers[dst as usize] = chunk.consts[konst as usize].clone();
            }

            Op::Move { dst, src, .. } => {
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
                    .map_err(|fault| Stopped { fault: *fault, span: spans[here] })?;
                registers[dst as usize] = Value::Num { ty, bits };
            }

            Op::Binary { op, dst, lhs, rhs, .. } => {
                let value = binary_op(
                    op,
                    &registers[lhs as usize],
                    &registers[rhs as usize],
                    chunk.overflow,
                )
                .map_err(|fault| Stopped { fault: *fault, span: spans[here] })?;
                registers[dst as usize] = value;
            }

            Op::Compare { op, operands, dst, lhs, rhs } if operands.is_integer() => {
                let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) =
                    (&registers[lhs as usize], &registers[rhs as usize])
                else {
                    unreachable!("an integer comparison has integers")
                };
                registers[dst as usize] =
                    Value::Bool(holds(op, int_compare(operands, *a, *b)));
            }

            Op::Compare { op, dst, lhs, rhs, .. } => {
                let ordering = compare(&registers[lhs as usize], &registers[rhs as usize]);
                registers[dst as usize] = Value::Bool(holds(op, ordering));
            }

            Op::Neg { dst, src, .. } => {
                let value = negate(&registers[src as usize], chunk.overflow)
                    .map_err(|fault| Stopped { fault: *fault, span: spans[here] })?;
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

            Op::PrintValue { src, .. } => {
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

            // The whole point of the type being on the instruction: an integer
            // comparison is a machine comparison, not an inspection of two values.
            Op::JumpIfGreater { lhs, rhs, ty, target } if ty.is_integer() => {
                let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) =
                    (&registers[lhs as usize], &registers[rhs as usize])
                else {
                    unreachable!("an integer comparison has integers")
                };
                if int_compare(ty, *a, *b) == Comparison::Greater {
                    at = target as usize;
                }
            }

            Op::JumpIfEqual { lhs, rhs, ty, target } if ty.is_integer() => {
                let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) =
                    (&registers[lhs as usize], &registers[rhs as usize])
                else {
                    unreachable!("an integer comparison has integers")
                };
                if a == b {
                    at = target as usize;
                }
            }

            Op::JumpIfGreater { lhs, rhs, target, .. } => {
                if compare(&registers[lhs as usize], &registers[rhs as usize])
                    == Comparison::Greater
                {
                    at = target as usize;
                }
            }

            Op::JumpIfEqual { lhs, rhs, target, .. } => {
                if compare(&registers[lhs as usize], &registers[rhs as usize])
                    == Comparison::Equal
                {
                    at = target as usize;
                }
            }
                }
            }
        };

        match step {
            Step::Called { func, fresh, dst } => {
                frames.last_mut().expect("a frame is always open").at = at;
                frames.push(Frame { routine: Some(func), at: 0, registers: fresh, dst });
            }
            Step::Returned(answer) => {
                let finished = frames.pop().expect("a frame is always open");
                if let Some(answer) = answer {
                    let caller = frames.last_mut().expect("something called it");
                    caller.registers[finished.dst as usize] = answer;
                }
            }
        }
        continue 'activation;
    }
}

/// What ended a run of instructions: something that changes which frame is running.
enum Step {
    Called { func: usize, fresh: Vec<Value>, dst: u16 },
    Returned(Option<Value>),
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

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

use luarust_core::{BinOp, Ty};
use luarust_core::heap;
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

/// The instruction the machine actually steps, as opposed to the one the chunk stores.
///
/// The one hot case — integer arithmetic — is split so the operation is an opcode
/// rather than a field. Stored as `Binary { op, .. }`, the operation is data, and
/// x86-64 pays a second dispatch inside `int_op` for every single instruction; split,
/// the outer match lands on an arm where the operation is a constant, and the inner
/// match folds away at compile time. The arithmetic itself is still `int_op`, so this
/// changes where a decision is made and not what any answer is. The chunk format does
/// not know this type exists.
#[derive(Clone, Copy)]
enum Micro {
    Add { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Sub { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mul { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Div { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mod { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Other(Op),
}

/// One-for-one, so every jump target and span index survives unchanged.
fn widen(code: &[Op]) -> Vec<Micro> {
    code.iter()
        .map(|&op| match op {
            Op::Binary { op: BinOp::Add, ty, dst, lhs, rhs } if ty.is_integer() => {
                Micro::Add { ty, dst, lhs, rhs }
            }
            Op::Binary { op: BinOp::Sub, ty, dst, lhs, rhs } if ty.is_integer() => {
                Micro::Sub { ty, dst, lhs, rhs }
            }
            Op::Binary { op: BinOp::Mul, ty, dst, lhs, rhs } if ty.is_integer() => {
                Micro::Mul { ty, dst, lhs, rhs }
            }
            Op::Binary { op: BinOp::Div, ty, dst, lhs, rhs } if ty.is_integer() => {
                Micro::Div { ty, dst, lhs, rhs }
            }
            Op::Binary { op: BinOp::Mod, ty, dst, lhs, rhs } if ty.is_integer() => {
                Micro::Mod { ty, dst, lhs, rhs }
            }
            other => Micro::Other(other),
        })
        .collect()
}

/// The integer-arithmetic step, once per [`Micro`] opcode, so the operation reaches
/// `int_op` as a constant and the dispatch on it disappears into the code.
macro_rules! int_arm {
    ($binop:expr, $ty:expr, $dst:expr, $lhs:expr, $rhs:expr,
     $registers:expr, $spans:expr, $here:expr, $overflow:expr) => {{
        let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) =
            (&$registers[$lhs as usize], &$registers[$rhs as usize])
        else {
            return Err(Stopped {
                fault: not_as_described("this says it works on whole numbers"),
                span: $spans[$here],
            });
        };
        let bits = int_op($binop, $ty, *a, *b, $overflow)
            .map_err(|fault| Stopped { fault, span: $spans[$here] })?;
        $registers[$dst as usize] = Value::Num { ty: $ty, bits };
    }};
}

/// Run a compiled chunk.
pub fn run(chunk: &Chunk, out: &mut impl Write) -> Result<(), Stopped> {
    heap::clear();
    // A chunk carries what its project decided, so running one applies it. Nothing else
    // has to be told, and `luarust-run` -- which has no project file and no way to read
    // one -- behaves as the project said.
    heap::set_threshold(chunk.collect.threshold());
    luarust_core::value::set_floats(chunk.floats);
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

    // The top level always runs, so it is translated up front; a routine is translated
    // the first time something enters it — the same reasoning that keeps the JIT from
    // compiling what nothing calls, at a much smaller price.
    let top = widen(&chunk.code);
    let mut routines: Vec<Option<Vec<Micro>>> = vec![None; chunk.funcs.len()];

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
            None => (&top[..], &chunk.spans[..]),
            Some(index) => {
                if routines[index].is_none() {
                    routines[index] = Some(widen(&chunk.funcs[index].code));
                }
                (
                    &routines[index].as_ref().expect("filled just above")[..],
                    &chunk.funcs[index].spans[..],
                )
            }
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
            Micro::Add { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Add, ty, dst, lhs, rhs, registers, spans, here, chunk.overflow)
            }
            Micro::Sub { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Sub, ty, dst, lhs, rhs, registers, spans, here, chunk.overflow)
            }
            Micro::Mul { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Mul, ty, dst, lhs, rhs, registers, spans, here, chunk.overflow)
            }
            Micro::Div { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Div, ty, dst, lhs, rhs, registers, spans, here, chunk.overflow)
            }
            Micro::Mod { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Mod, ty, dst, lhs, rhs, registers, spans, here, chunk.overflow)
            }

            Micro::Other(op) => match op {
            Op::Halt => return Ok(()),

            Op::Call { func, base, argc, dst } => {
                if depth >= DEPTH_LIMIT {
                    return Err(Stopped {
                        fault: Box::new(Fault {
                            code: "R0011",
                            message: format!("this has called itself {DEPTH_LIMIT} deep."),
                            rule: "a call may only go so deep before the program is stopped",
                            fix: "give the recursion a case that stops, or write it as a loop."
                                .to_string(),
                        }),
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
                    return Err(Stopped {
                        fault: not_as_described("this says it works on whole numbers"),
                        span: spans[here],
                    });
                };
                let bits = int_op(op, ty, *a, *b, chunk.overflow)
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                registers[dst as usize] = Value::Num { ty, bits };
            }

            Op::Binary { op, dst, lhs, rhs, .. } => {
                let value = binary_op(
                    op,
                    &registers[lhs as usize],
                    &registers[rhs as usize],
                    chunk.overflow,
                )
                .map_err(|fault| Stopped { fault, span: spans[here] })?;
                registers[dst as usize] = value;
            }

            Op::Compare { op, operands, dst, lhs, rhs } if operands.is_integer() => {
                let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) =
                    (&registers[lhs as usize], &registers[rhs as usize])
                else {
                    return Err(Stopped {
                        fault: not_as_described("this says it compares whole numbers"),
                        span: spans[here],
                    });
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
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                registers[dst as usize] = value;
            }

            Op::TimeNow { dst, ty } => {
                let seconds = started.elapsed().as_secs_f64();
                let Some(fmt) = format_of(ty) else {
                    return Err(Stopped {
                        fault: not_as_described("this says the clock is read as a float"),
                        span: spans[here],
                    });
                };
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

            Op::NewArray { dst, items, count, ty } => {
                let of = ty.array().expect("a new array has an array type");
                let held: Vec<Value> = (0..count as usize)
                    .map(|n| registers[items as usize + n].clone())
                    .collect();
                registers[dst as usize] = heap::handle(ty, heap::of(of.element, &held));
                if heap::wants_collecting() {
                    break Step::Collecting;
                }
            }

            Op::Filled { dst, length, value, ty } => {
                let of = ty.array().expect("a filled array has an array type");
                let count = registers[length as usize].as_i128().unwrap_or(0);
                if count < 0 {
                    return Err(Stopped { fault: fewer_than_none(), span: spans[here] });
                }
                let fill = registers[value as usize].clone();
                registers[dst as usize] =
                    heap::handle(ty, heap::make(of.element, count as usize, &fill));
                if heap::wants_collecting() {
                    break Step::Collecting;
                }
            }

            Op::At { dst, array, at, rank, ty } => {
                let Some(handle) = handle_of(&registers[array as usize]) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it indexes an array"),
                        span: spans[here],
                    });
                };
                let index = offset(ty, handle, registers, at, rank)
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                registers[dst as usize] = heap::read(handle, index).ok_or(Stopped {
                    fault: out_of_range(index as i128 + 1, heap::length(handle) as i128),
                    span: spans[here],
                })?;
            }

            Op::StoreAt { array, at, rank, value, ty } => {
                let Some(handle) = handle_of(&registers[array as usize]) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it indexes an array"),
                        span: spans[here],
                    });
                };
                let index = offset(ty, handle, registers, at, rank)
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                let held = registers[value as usize].clone();
                heap::store(handle, index, &held);
            }

            Op::Count { dst, array, ty } => {
                let Some(handle) = handle_of(&registers[array as usize]) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it indexes an array"),
                        span: spans[here],
                    });
                };
                registers[dst as usize] =
                    Value::Num { ty, bits: heap::length(handle) as u64 };
            }

            Op::Not { dst, src } => {
                let Some(answer) = truth(&registers[src as usize]) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it negates a truth"),
                        span: spans[here],
                    });
                };
                registers[dst as usize] = Value::Bool(!answer);
            }

            Op::JumpIfFalse { cond, target } => {
                let Some(answer) = truth(&registers[cond as usize]) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it branches on a truth"),
                        span: spans[here],
                    });
                };
                if !answer {
                    at = target as usize;
                }
            }

            Op::JumpIfTrue { cond, target } => {
                let Some(answer) = truth(&registers[cond as usize]) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it branches on a truth"),
                        span: spans[here],
                    });
                };
                if answer {
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
                    return Err(Stopped {
                        fault: not_as_described("this says it compares whole numbers"),
                        span: spans[here],
                    });
                };
                if int_compare(ty, *a, *b) == Comparison::Greater {
                    at = target as usize;
                }
            }

            Op::JumpIfEqual { lhs, rhs, ty, target } if ty.is_integer() => {
                let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) =
                    (&registers[lhs as usize], &registers[rhs as usize])
                else {
                    return Err(Stopped {
                        fault: not_as_described("this says it compares whole numbers"),
                        span: spans[here],
                    });
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
                },
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
            Step::Collecting => {
                frames.last_mut().expect("a frame is always open").at = at;
                // Every register of every frame is a root. A register the checker has
                // proved is written before it is read may still hold the placeholder, and
                // a `bool` is not a handle, so nothing false is kept alive by looking.
                heap::collect(frames.iter().flat_map(|frame| frame.registers.iter()));
            }
        }
        continue 'activation;
    }
}

/// What ended a run of instructions: something that changes which frame is running.
enum Step {
    Called { func: usize, fresh: Vec<Value>, dst: u16 },
    Returned(Option<Value>),
    /// The heap has asked to be collected, which cannot happen in here: the roots are
    /// every frame's registers and this loop is holding one frame's borrowed.
    Collecting,
}

/// What a condition answered. The checker refuses anything that is not a `bool` long
/// before a chunk exists, and a chunk read off disk has its own check, so there is
/// nothing to decide here.
fn truth(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(answer) => Some(*answer),
        // Not a `bool`, which the checker makes impossible and a corrupt chunk does not.
        _ => None,
    }
}

/// The array a value points at.
fn handle_of(value: &Value) -> Option<u32> {
    match value {
        Value::Num { bits, .. } => Some(*bits as u32),
        _ => None,
    }
}

/// Where an index lands, counted from one and flattened row by row.
fn offset(
    ty: Ty,
    handle: u32,
    registers: &[Value],
    at: u16,
    rank: u8,
) -> Result<usize, Box<Fault>> {
    let of = ty.array().expect("only an array is indexed");
    let dims = of.dims();
    let mut flat = 0usize;

    for place in 0..rank as usize {
        let held = registers[at as usize + place].as_i128().unwrap_or(0);
        let past = if dims.is_empty() {
            heap::length(handle) as i128
        } else {
            i128::from(dims[place])
        };
        if held < 1 || held > past {
            return Err(out_of_range(held, past));
        }
        flat = flat * dims.get(place).copied().unwrap_or(1) as usize + (held as usize - 1);
    }
    Ok(flat)
}

/// Reaching for an element that is not there.
fn out_of_range(at: i128, length: i128) -> Box<Fault> {
    Box::new(Fault::of(
        "R0015",
        format!("there is no element {at} here."),
        "an array is counted from one, up to how many it holds",
        if length == 0 {
            "this one holds nothing at all.".to_string()
        } else {
            format!("this one holds {length}, so the last is {length} and the first is 1.")
        },
    ))
}

/// A chunk whose instruction disagrees with what its registers hold.
///
/// The compiler never writes one. A file that arrived from somewhere else may say
/// anything, and every other field it contains is checked against something it indexes —
/// a type tag indexes nothing, so nothing at load can tell a `ui8` tag from a `bool` tag.
/// This is where the disagreement finally shows, and it is a broken file rather than a
/// broken program, so it says so.
fn not_as_described(saying: &str) -> Box<Fault> {
    Box::new(Fault::of(
        "R0016",
        "this chunk does not hold what it says it holds.",
        "an instruction's type is the type of the values it works on",
        format!("rebuild it from the source: {saying}"),
    ))
}

fn fewer_than_none() -> Box<Fault> {
    Box::new(Fault::of(
        "R0014",
        "this asks for an array of fewer than no elements.",
        "an array holds none or more",
        "give it a length of nought or more.",
    ))
}

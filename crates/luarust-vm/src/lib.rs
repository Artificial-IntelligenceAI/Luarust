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
/// Whether a chunk's instructions agree with its registers, established at load.
pub mod typed;

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


/// How many times a loop goes round before it is worth compiling.
///
/// Low enough that anything with real work in it trips almost at once -- a loop reaching
/// ten thousand iterations has already spent more time interpreting than LLVM will spend
/// compiling it -- and high enough that a loop counting to a hundred is left alone, since
/// compiling that would cost more than running it ever could.
pub const HOT: u32 = 10_000;

/// Something that can take a hot loop off the VM's hands.
///
/// The VM cannot call the JIT itself: the JIT reads chunks, so it depends on this crate,
/// and a dependency the other way would close the circle. Handing it out as a trait is
/// also what lets `luarust-run` stay what it is -- a runtime with no compiler in it
/// installs nothing here and never tiers, which is why it can be a few hundred kilobytes
/// while the toolchain with LLVM in it is thirty megabytes.
pub trait Tier {
    /// How many times round before a loop is worth compiling. [`HOT`] unless something
    /// knows better -- a test that wants the switch to happen says so here rather than
    /// running ten thousand iterations to earn it.
    fn threshold(&self) -> u32 {
        HOT
    }

    /// The loop beginning at `at` has gone round [`threshold`](Tier::threshold) times.
    /// Take it over if you can.
    ///
    /// `routine` says which code `at` is an instruction of -- `None` for the top level.
    /// The two are not the same job: taking over the top level means running to the end of
    /// the program, and taking over a routine means running it to its return and giving the
    /// answer back.
    ///
    /// `frames` is every call the VM has open, outermost first, with the live registers of
    /// each. Whatever takes over must start from exactly those values, and all of them are
    /// handed over rather than only the hot one: they are the root set a collection walks,
    /// and they are what the depth limit counts. `started` is when the program started,
    /// which is not when this was asked -- a clock that reset itself here would be
    /// reporting on the compiler rather than on the program. Anything printed goes to
    /// `out`, after what the VM has already printed.
    fn hot(
        &mut self,
        chunk: &Chunk,
        routine: Option<usize>,
        at: usize,
        frames: Vec<Vec<Value>>,
        started: Instant,
        out: &mut dyn Write,
    ) -> Taken;

    /// Whether machine code for `routine` is being kept. Asked before every interpreted
    /// call, so it has to cost a lookup and nothing more — the cloning a handover needs
    /// only happens once this says yes.
    fn keeps(&self, routine: usize) -> bool {
        let _ = routine;
        false
    }

    /// Run one call of `routine` on kept machine code, instead of the VM interpreting
    /// it. Only ever asked after [`keeps`](Tier::keeps) said yes: a tier that keeps a
    /// routine answers every call of it, so there is no way to decline here.
    ///
    /// `open` is every call the VM has open, outermost first, borrowed for exactly as
    /// long as this call runs — with `fresh`, the frame the call runs on, they are the
    /// root set and what the depth limit counts. The answer is what the routine
    /// returned, exactly as [`Taken::Returned`] carries it.
    fn call(
        &mut self,
        chunk: &Chunk,
        routine: usize,
        open: &[&Vec<Value>],
        fresh: Vec<Value>,
        started: Instant,
        out: &mut dyn Write,
    ) -> Result<Option<Value>, Stopped> {
        let _ = (chunk, routine, open, fresh, started, out);
        unreachable!("the VM only asks about a routine `keeps` said it was keeping");
    }
}

/// What a [`Tier`] did with a hot loop.
pub enum Taken {
    /// Nothing. The VM carries on, and is not asked about this loop again.
    Declined,
    /// Compiled code ran from the loop head to the end of the program, and this is how it
    /// ended. There is nothing for the VM to resume: entering at a loop head means
    /// running everything that follows it, the loop's own exit included.
    Finished(Result<(), Stopped>),
    /// Compiled code ran a routine from the loop head to its return, and this is what it
    /// gave back. The VM pops the frame and carries on interpreting the call underneath,
    /// which never stopped waiting.
    Returned(Result<Option<Value>, Stopped>),
}

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
    /// A jump backwards, and the counter that says how often it has been taken. Only ever
    /// made when something is listening for it, so the ordinary VM pays nothing for a
    /// tiering machinery it is not using.
    Back { target: u32, counter: u32 },
    Add { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Sub { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mul { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Div { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mod { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    /// Reading one element, with the array's shape already looked up.
    ///
    /// `Op::At` carries a type, and a type only *names* a shape -- finding the real one
    /// means a table, behind a thread local, behind a `RefCell`, and then copying the
    /// whole thing out. That happened on every element read, to fetch something that
    /// cannot change while a program runs. It is fetched here instead, once, when the
    /// instruction is first widened.
    At { dst: chunk::Reg, array: chunk::Reg, at: u16, rank: u8, shape: u8 },
    /// A counting loop's whole tail — leave if the counter reached the bound, otherwise
    /// step it and go round — fetched and dispatched once where the chunk says three.
    /// Only ever made from the exact shape `compile` emits, when nothing jumps into the
    /// middle of it; the swallowed add and jump stay where they were, unreachable, so
    /// every other target and span survives unchanged. The jump back is read from the
    /// slot beside it rather than carried, which is what keeps this no wider than `Op`.
    Tail { ty: Ty, counter: chunk::Reg, limit: chunk::Reg, step: chunk::Reg },
    Other(Op),
}

/// One-for-one, so every jump target and span index survives unchanged.
///
/// `fuse` folds each counting loop's three-instruction tail into one [`Micro::Tail`],
/// and is off when back edges are being counted — a fused tail has no separate jump
/// for a counter to ride on, and the tiering engine wants every one.
fn widen(code: &[Op], mut counters: Option<&mut Vec<u32>>, fuse: bool) -> Vec<Micro> {
    // Where jumps land. A fused tail swallows the two instructions after it, which is
    // only sound while nothing else can arrive at either.
    let mut landed = vec![false; code.len()];
    for op in code {
        if let Op::Jump { target }
        | Op::JumpIfFalse { target, .. }
        | Op::JumpIfTrue { target, .. }
        | Op::JumpIfGreater { target, .. }
        | Op::JumpIfEqual { target, .. } = op
            && let Some(seen) = landed.get_mut(*target as usize)
        {
            *seen = true;
        }
    }
    code.iter()
        .enumerate()
        .map(|(here, &op)| match op {
            // The tail `compile` emits for a counting loop, whole: leave when the
            // counter has reached the bound, step it otherwise, and go round again.
            Op::JumpIfEqual { lhs, rhs, ty, target } if fuse
                && counters.is_none()
                && ty.is_integer()
                && target as usize == here + 3
                && !landed[here + 1]
                && !landed[here + 2]
                && matches!(
                    code[here + 1],
                    Op::Binary { op: BinOp::Add, ty: add_ty, dst, lhs: add_lhs, .. }
                        if add_ty == ty && dst == lhs && add_lhs == lhs
                )
                && matches!(code[here + 2], Op::Jump { target: top } if top as usize <= here) =>
            {
                let Op::Binary { rhs: step, .. } = code[here + 1] else {
                    unreachable!("just matched an add");
                };
                Micro::Tail { ty, counter: lhs, limit: rhs, step }
            }
            // A loop is a jump backwards, so a back edge is a jump whose target is at or
            // behind it, and counting those counts iterations without touching anything
            // else. The counter is made here rather than looked up there: by the time the
            // machine is running, which back edge this is has already been decided.
            Op::Jump { target } if counters.is_some() && target as usize <= here => {
                let counters = counters.as_mut().expect("just tested");
                let counter = counters.len() as u32;
                counters.push(0);
                Micro::Back { target, counter }
            }
            Op::At { dst, array, at, rank, ty } => {
                let Ty::Array(shape) = ty else { return Micro::Other(op) };
                Micro::At { dst, array, at, rank, shape }
            }
            Op::Binary { op: BinOp::Add, ty, dst, lhs, rhs, .. } if ty.is_integer() => {
                Micro::Add { ty, dst, lhs, rhs }
            }
            Op::Binary { op: BinOp::Sub, ty, dst, lhs, rhs, .. } if ty.is_integer() => {
                Micro::Sub { ty, dst, lhs, rhs }
            }
            Op::Binary { op: BinOp::Mul, ty, dst, lhs, rhs, .. } if ty.is_integer() => {
                Micro::Mul { ty, dst, lhs, rhs }
            }
            Op::Binary { op: BinOp::Div, ty, dst, lhs, rhs, .. } if ty.is_integer() => {
                Micro::Div { ty, dst, lhs, rhs }
            }
            Op::Binary { op: BinOp::Mod, ty, dst, lhs, rhs, .. } if ty.is_integer() => {
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
        // Written a field at a time when the slot already holds a number, which it
        // almost always does: a whole-value write is a 16-byte vector store, and the
        // narrow loads that read it back next instruction stall on x86-64 until it
        // drains to cache — measured at 12x the matched-width cost on Zen 3, and the
        // single largest reason the VM was slower there than the machine explains.
        if let Value::Num { ty: t, bits: b } = &mut $registers[$dst as usize] {
            *t = $ty;
            *b = bits;
        } else {
            $registers[$dst as usize] = Value::Num { ty: $ty, bits };
        }
    }};
}

/// Run a compiled chunk.
pub fn run(chunk: &Chunk, out: &mut impl Write) -> Result<(), Stopped> {
    run_with(chunk, out, None)
}

/// Run a chunk, with something that may take its hot loops.
///
/// Passing `None` is [`run`], and is what a build with no compiler in it can do. Passing
/// a [`Tier`] is the tiering engine: interpreted until a loop proves itself, compiled from
/// then on.
pub fn run_with(
    chunk: &Chunk,
    out: &mut impl Write,
    tier: Option<&mut dyn Tier>,
) -> Result<(), Stopped> {
    run_widened(chunk, out, tier, true)
}

/// [`run_with`], saying whether loop tails are fused — one binary holding both
/// arrangements is the only instrument that can compare them, since two builds of
/// identical source have measured 8% apart on code layout alone.
fn run_widened(
    chunk: &Chunk,
    out: &mut impl Write,
    mut tier: Option<&mut dyn Tier>,
    fuse: bool,
) -> Result<(), Stopped> {
    heap::clear();
    // A chunk carries what its project decided, so running one applies it. Nothing else
    // has to be told, and `luarust-run` -- which has no project file and no way to read
    // one -- behaves as the project said.
    heap::set_threshold(chunk.collect.threshold());
    luarust_core::value::set_floats(chunk.floats);
    luarust_core::value::set_division(chunk.division);
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
    // One flat set of counters for the whole run, handed out as code is widened: the top
    // level up front and each routine the first time something enters it. Nothing is
    // counted at all when nothing is listening, which is what keeps the plain VM paying
    // nothing for machinery it is not using.
    // Every array shape the program named, taken once. Asking for one per element read
    // is a thread local, a `RefCell` and a copy, for something that cannot change while a
    // program runs — and putting the dimensions in the instruction instead made every
    // instruction fetch in the whole machine eight bytes heavier, which the arithmetic
    // pays for too.
    let shapes = luarust_core::ty::shapes();
    let mut counters: Vec<u32> = Vec::new();
    let (top, threshold) = match &tier {
        // Never nought: a counter is compared after it is raised, so a threshold nothing
        // could reach would be a loop that compiles before it has gone round at all.
        Some(tier) => (widen(&chunk.code, Some(&mut counters), fuse), tier.threshold().max(1)),
        None => (widen(&chunk.code, None, fuse), 0),
    };
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
                    let counted = match tier {
                        Some(_) => widen(&chunk.funcs[index].code, Some(&mut counters), fuse),
                        None => widen(&chunk.funcs[index].code, None, fuse),
                    };
                    routines[index] = Some(counted);
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
            Micro::Back { target, counter } => {
                // Exactly once, at the threshold: a counter that has fired is pushed past
                // it and saturates there, so a loop nobody wanted is never asked about
                // twice.
                let hits = &mut counters[counter as usize];
                *hits = hits.saturating_add(1);
                if *hits == threshold {
                    break Step::Hot { target: target as usize, counter };
                }
                at = target as usize;
            }
            Micro::At { dst, array, at, rank, shape } => {
                let Some(handle) = handle_of(&registers[array as usize]) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it indexes an array"),
                        span: spans[here],
                    });
                };
                let index = offset(shapes[shape as usize].dims(), handle, registers, at, rank)
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                let held = heap::read(handle, index).ok_or_else(|| Stopped {
                    fault: out_of_range(index as i128 + 1, heap::length(handle) as i128),
                    span: spans[here],
                })?;
                // A field at a time, for the reason the arithmetic does it: an array of
                // numbers read in a loop writes the same register every pass, and a
                // whole-value write is a sixteen-byte store that the next instruction's
                // narrow load waits on.
                match (&mut registers[dst as usize], held) {
                    (Value::Num { ty: t, bits: b }, Value::Num { ty, bits }) => {
                        *t = ty;
                        *b = bits;
                    }
                    (slot, held) => *slot = held,
                }
            }
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

            Micro::Tail { ty, counter, limit, step } => {
                let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) =
                    (&registers[counter as usize], &registers[limit as usize])
                else {
                    return Err(Stopped {
                        fault: not_as_described("this says it compares whole numbers"),
                        span: spans[here],
                    });
                };
                if a == b {
                    // Past the swallowed add and jump, to where the loop lands.
                    at += 2;
                } else {
                    int_arm!(
                        BinOp::Add, ty, counter, counter, step, registers, spans, here,
                        chunk.overflow
                    );
                    // The jump back was left in the slot beside this one, unreachable
                    // except from here.
                    let Micro::Other(Op::Jump { target }) = code[at + 1] else {
                        unreachable!("a fused tail keeps its jump beside it");
                    };
                    at = target as usize;
                }
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
            Op::Binary { op, ty, dst, lhs, rhs, .. } if ty.is_integer() => {
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

            // Widened into `Micro::At` before anything runs, so it cannot arrive here.
            // Left as a case rather than a catch-all: if a future `widen` stops handling
            // it, this stops compiling instead of quietly costing a shape lookup an
            // element again.
            Op::At { .. } => unreachable!("every `At` is widened"),

            Op::StoreAt { array, at, rank, value, ty } => {
                let Some(handle) = handle_of(&registers[array as usize]) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it indexes an array"),
                        span: spans[here],
                    });
                };
                let shape = ty.array().expect("only an array is indexed");
                let index = offset(shape.dims(), handle, registers, at, rank)
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                let held = registers[value as usize].clone();
                // The answer is not decoration. `offset` no longer checks the upper bound
                // of an array that grows -- the heap knows the real length and is about to
                // look anyway -- so this is where writing past the end is caught. It was
                // discarded here for as long as `offset` caught it first, and the moment
                // that stopped being true an out-of-range write became a silent no-op.
                // The fuzzer found it on seed 7894.
                if !heap::store(handle, index, &held) {
                    return Err(Stopped {
                        fault: out_of_range(index as i128 + 1, heap::length(handle) as i128),
                        span: spans[here],
                    });
                }
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
                // Machine code first, when some is being kept for this routine: the
                // open frames are lent rather than copied — the VM is suspended right
                // here until the call comes back — and the answer lands exactly where
                // a `return` would have put it. The frame is never pushed, because the
                // call never runs here.
                if let Some(t) = tier.as_deref_mut()
                    && t.keeps(func)
                {
                    let open: Vec<&Vec<Value>> =
                        frames.iter().map(|frame| &frame.registers).collect();
                    let answer = t.call(chunk, func, &open, fresh, started, out)?;
                    if let Some(answer) = answer {
                        let caller = frames.last_mut().expect("something called it");
                        caller.registers[dst as usize] = answer;
                    }
                    continue 'activation;
                }
                frames.push(Frame { routine: Some(func), at: 0, registers: fresh, dst });
            }
            Step::Returned(answer) => {
                let finished = frames.pop().expect("a frame is always open");
                if let Some(answer) = answer {
                    let caller = frames.last_mut().expect("something called it");
                    caller.registers[finished.dst as usize] = answer;
                }
            }
            Step::Hot { target, counter } => {
                frames.last_mut().expect("a frame is always open").at = target;
                // Never asked about again, whatever the answer: either compiled code took
                // it, or there is no compiled code to be had.
                counters[counter as usize] = u32::MAX;
                let Some(tier) = tier.as_deref_mut() else { continue 'activation };
                // Every open call, outermost first. Cloned because compiled code is about
                // to work on its own copy and the VM's is what it comes back to.
                let open: Vec<Vec<Value>> =
                    frames.iter().map(|frame| frame.registers.clone()).collect();
                match tier.hot(chunk, routine, target, open, started, out) {
                    Taken::Declined => {}
                    Taken::Finished(outcome) => return outcome,
                    Taken::Returned(Err(stopped)) => return Err(stopped),
                    // The same thing a `return` does, because that is what happened: the
                    // routine ran to its end, somewhere else.
                    Taken::Returned(Ok(answer)) => {
                        let finished = frames.pop().expect("a frame is always open");
                        if let Some(answer) = answer {
                            let caller = frames.last_mut().expect("something called it");
                            caller.registers[finished.dst as usize] = answer;
                        }
                    }
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
    /// A loop has gone round enough times to be worth compiling. Same reason it is
    /// settled out there: handing the frame over needs it unborrowed.
    Hot { target: usize, counter: u32 },
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
    dims: &[u32],
    handle: u32,
    registers: &[Value],
    at: u16,
    rank: u8,
) -> Result<usize, Box<Fault>> {
    let dims = &dims[..dims.iter().position(|d| *d == 0).unwrap_or(dims.len())];
    let mut flat = 0usize;

    for place in 0..rank as usize {
        // The index at sixty-four bits, which is every width this language has. It was
        // `as_i128`, and widening cost a match, a pair of 128-bit shifts and 128-bit
        // comparisons on every element read, to hold a number that never needed the room.
        // The same mistake as `Division::apply` made, in a hotter place.
        let held: i64 = match &registers[at as usize + place] {
            Value::Num { ty, bits } if ty.is_signed() => {
                let shift = 64 - ty.int_bits().unwrap_or(64);
                ((*bits << shift) as i64) >> shift
            }
            // An unsigned index past `i64::MAX` is past the end of any array there could
            // be, so holding it down keeps the comparison in one register. The fault path
            // reads the real value again to report it.
            Value::Num { bits, .. } => (*bits).min(i64::MAX as u64) as i64,
            _ => 0,
        };
        // A shaped array knows its bounds without asking. One that grows does not -- and
        // `heap::read` is about to check the index against the real length anyway, so
        // asking here as well meant two trips through the heap for every element read,
        // the second of them computing a number only the error path ever looks at. Too
        // small an index is still caught here, because that one nothing downstream sees.
        let past = (!dims.is_empty()).then(|| i64::from(dims[place]));
        if held < 1 || past.is_some_and(|past| held > past) {
            let reported = registers[at as usize + place].as_i128().unwrap_or(0);
            return Err(out_of_range(
                reported,
                past.map_or_else(|| heap::length(handle) as i128, i128::from),
            ));
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

#[cfg(test)]
mod sizes {
    #[test]
    fn a_value_is_not_wider_than_a_number_and_its_type() {
        // Every register write moves one of these. The widest variant decides it for all
        // of them, which is why the string is a thin pointer -- see `Value::Str`.
        let value = std::mem::size_of::<luarust_core::value::Value>();
        assert!(value <= 16, "a Value is {value} bytes");
    }

    #[test]
    fn micro_is_not_bigger_than_the_op_it_widens() {
        // Every instruction fetch loads one of these, so its size is a cost the whole
        // dispatch loop pays whether or not the wide variant is the one running.
        let micro = std::mem::size_of::<super::Micro>();
        let op = std::mem::size_of::<crate::Op>();
        assert!(micro <= op, "Micro is {micro} bytes against Op's {op}");
    }

    #[cfg(feature = "compile")]
    fn chunk_of(source: &str) -> crate::Chunk {
        let lexed = luarust_lex::lex(source);
        assert!(lexed.ok(), "lexing failed: {:#?}", lexed.errors);
        let parsed = luarust_parse::parse(source, &lexed.tokens);
        assert!(parsed.ok(), "parsing failed: {:#?}", parsed.errors);
        let (program, errors) = luarust_check::check(&parsed.program);
        assert!(errors.is_empty(), "checking failed: {errors:#?}");
        crate::compile(&program)
    }

    #[cfg(feature = "compile")]
    #[test]
    fn a_fused_tail_changes_no_answer() {
        // Loops that leave from the middle, loops that never run, a loop right at the
        // top of its type, and nested ones — the shapes where a fused tail could get
        // an exit wrong.
        for source in [
            "var.local.mut.i64 ['sum'] = [|0|];\n\
             loop.temp.range.i64 ['i'] = [|1|, |10|] {\n\
             set ['sum'] = [math { ('sum' + 'i') mod 7 }];\n\
             }\nprint['sum' \\n];\n",
            "var.local.mut.i64 ['x'] = [|5|];\n\
             loop.temp.range.i64 ['i'] = [|1|, |10|] {\n\
             if [math { 'i' = 3 }] { set ['x'] = [|-1|]; break; }\n\
             set ['x'] = [|9|];\n\
             }\nprint['x' \\n];\n",
            "var.local.mut.i64 ['n'] = [|0|];\n\
             loop.temp.range.i64 ['i'] = [|5|, |4|] { set ['n'] = [|1|]; }\n\
             print['n' \\n];\n",
            "var.local.mut.ui8 ['n'] = [|0|];\n\
             loop.temp.range.ui8 ['i'] = [|250|, |255|] { set ['n'] = ['i']; }\n\
             print['n' \\n];\n",
            "var.local.mut.i64 ['s'] = [|0|];\n\
             loop.temp.range.i64 ['i'] = [|1|, |4|] {\n\
             loop.temp.range.i64 ['j'] = [|1|, |3|] {\n\
             set ['s'] = [math { 's' + 1 }];\n\
             }\n\
             }\nprint['s' \\n];\n",
        ] {
            let chunk = chunk_of(source);
            let mut fused = Vec::new();
            let mut plain = Vec::new();
            let a = crate::run_widened(&chunk, &mut fused, None, true);
            let b = crate::run_widened(&chunk, &mut plain, None, false);
            assert_eq!(a.is_ok(), b.is_ok(), "the two arrangements ended differently");
            assert_eq!(
                String::from_utf8_lossy(&fused),
                String::from_utf8_lossy(&plain),
                "the two arrangements printed differently\n\n{source}"
            );
        }
    }

    /// The instrument, not a test: both arrangements timed from one binary, so code
    /// layout — worth 8% between two builds of identical source — cancels out.
    #[cfg(feature = "compile")]
    #[test]
    #[ignore = "a measurement, run by hand with --nocapture"]
    fn tails_fused_against_not() {
        let add = "var.local.mut.i64 ['sum'] = [|0|];\n\
                   loop.temp.range.i64 ['i'] = [|1|, |30000000|] {\n\
                   set ['sum'] = [math { 'sum' + 'i' }];\n\
                   }\nprint['sum' \\n];\n";
        let array = "var.local.mut.array.i64 ['xs'] = [filled[|64|, |7|]];\n\
                     var.local.mut.i64 ['sum'] = [|0|];\n\
                     loop.temp.range.ui32 ['i'] = [|1|, |30000000|] {\n\
                     set ['sum'] = [math { 'sum' + 'xs'[math { (('i' - 1) mod 64) + 1 }] }];\n\
                     }\nprint['sum' \\n];\n";
        for (name, source) in [("add", add), ("array", array)] {
            let chunk = chunk_of(source);
            let (mut fused, mut plain) = (u128::MAX, u128::MAX);
            for _ in 0..6 {
                for (fuse, best) in [(true, &mut fused), (false, &mut plain)] {
                    let mut out = Vec::new();
                    let t0 = std::time::Instant::now();
                    crate::run_widened(&chunk, &mut out, None, fuse).expect("it runs");
                    *best = (*best).min(t0.elapsed().as_nanos());
                }
            }
            println!(
                "{name}: fused {} ms, unfused {} ms over 30M iterations \
                 ({:.2} against {:.2} ns an iteration)",
                fused / 1_000_000,
                plain / 1_000_000,
                fused as f64 / 30_000_000.0,
                plain as f64 / 30_000_000.0,
            );
        }
    }
}

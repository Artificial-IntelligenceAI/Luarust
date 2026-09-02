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
    DEPTH_LIMIT, Fault, Overflow, Stopped, Value, int_compare, binary_op, compare, format_of,
    holds, int_op, negate,
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
struct Frame<F> {
    /// The function being run, or `None` for the top level.
    routine: Option<usize>,
    at: usize,
    file: F,
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
    /// Arithmetic that wraps, at a width the opcode names rather than carries.
    ///
    /// `int_op` decides three things per instruction: which operation, which of eight
    /// integer types, and whether overflow traps. The first is already the variant. The
    /// other two are a ten-way match and a branch, run once per instruction executed, to
    /// settle what was settled when the chunk was compiled — and ablating them off the
    /// add loop took 2.73 ns an iteration to 1.92, thirty per cent.
    ///
    /// So `widen` settles them instead. A chunk says once whether overflow wraps, and an
    /// instruction says once what width it works at, so a chunk that wraps gets an
    /// opcode per width and the arm is one machine instruction and a mask. Signedness
    /// does not appear: wrapping add, subtract and multiply are the same bits either
    /// way, and only the overflow *check* told them apart — which a wrapping chunk does
    /// not make. Trapping chunks, and divide and remainder, keep the general arm.
    ///
    /// The type is still carried, unused by the arithmetic. A boxed register file builds
    /// a `Value` on every write and needs to know what it is making; the split one
    /// ignores it. Dropping it would save two bytes and cost the tiered path.
    Add8 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Add16 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Add32 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Add64 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Sub8 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Sub16 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Sub32 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Sub64 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mul8 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mul16 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mul32 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mul64 { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Add { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Sub { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mul { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Div { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    Mod { ty: Ty, dst: chunk::Reg, lhs: chunk::Reg, rhs: chunk::Reg },
    /// One element of a one-dimensional array, read where its index already lives.
    ///
    /// `compile` stages an index through a register of its own before every `At`, and
    /// that copy is load-bearing *for the compiled path*: without it LLVM derives the
    /// address from the loop counter and stops vectorising the loop, which is worth
    /// thirteen times on array work — see `compile::arguments`, where taking it out was
    /// tried and put back. The chunk therefore keeps it, and the JIT keeps its
    /// vectoriser. The VM has no such need and was paying a whole dispatched
    /// instruction per element for somebody else's benefit, so `widen` fuses the pair
    /// and the machine reads the index where the counter already holds it.
    ///
    /// The copy is not performed. Its destination is a temp `arguments` claimed and
    /// released, and the compiler never reads a temp it has not just written — the same
    /// standing `Tail` swallows its neighbours on, and the same gate proves it: a
    /// register left stale here is a wrong answer, which the fuzzer and the four-way
    /// sweep both compare against the tree-walker.
    AtOne { dst: chunk::Reg, array: chunk::Reg, from: chunk::Reg, shape: u8 },
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
fn widen(
    code: &[Op],
    mut counters: Option<&mut Vec<u32>>,
    fuse: bool,
    overflow: Overflow,
) -> Vec<Micro> {
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
            // The staging copy in front of a one-dimensional index, swallowed. Only
            // when nothing else can arrive at the `At` it swallows, and only off the
            // tiered path, which hands its registers to compiled code at a loop head and
            // wants one `Micro` per `Op` to do it.
            Op::Move { dst: staged, src: from, .. } if fuse
                && counters.is_none()
                && landed.get(here + 1) == Some(&false)
                && matches!(
                    code.get(here + 1),
                    Some(Op::At { at, rank: 1, ty: Ty::Array(_), .. }) if *at == staged
                ) =>
            {
                let Some(&Op::At { dst, array, ty: Ty::Array(shape), .. }) = code.get(here + 1)
                else {
                    unreachable!("just matched an at over an array");
                };
                Micro::AtOne { dst, array, from, shape }
            }
            Op::At { dst, array, at, rank, ty } => {
                let Ty::Array(shape) = ty else { return Micro::Other(op) };
                Micro::At { dst, array, at, rank, shape }
            }
            // A chunk that wraps has nothing left to decide about width or overflow, so
            // the opcode says both and the arm says neither.
            Op::Binary { op: kind, ty, dst, lhs, rhs, .. }
                if overflow == Overflow::Wrap
                    && matches!(kind, BinOp::Add | BinOp::Sub | BinOp::Mul)
                    && ty.is_integer() =>
            {
                match (kind, ty) {
                    (BinOp::Add, Ty::I8 | Ty::U8) => Micro::Add8 { ty, dst, lhs, rhs },
                    (BinOp::Add, Ty::I16 | Ty::U16) => Micro::Add16 { ty, dst, lhs, rhs },
                    (BinOp::Add, Ty::I32 | Ty::U32) => Micro::Add32 { ty, dst, lhs, rhs },
                    (BinOp::Add, _) => Micro::Add64 { ty, dst, lhs, rhs },
                    (BinOp::Sub, Ty::I8 | Ty::U8) => Micro::Sub8 { ty, dst, lhs, rhs },
                    (BinOp::Sub, Ty::I16 | Ty::U16) => Micro::Sub16 { ty, dst, lhs, rhs },
                    (BinOp::Sub, Ty::I32 | Ty::U32) => Micro::Sub32 { ty, dst, lhs, rhs },
                    (BinOp::Sub, _) => Micro::Sub64 { ty, dst, lhs, rhs },
                    (_, Ty::I8 | Ty::U8) => Micro::Mul8 { ty, dst, lhs, rhs },
                    (_, Ty::I16 | Ty::U16) => Micro::Mul16 { ty, dst, lhs, rhs },
                    (_, Ty::I32 | Ty::U32) => Micro::Mul32 { ty, dst, lhs, rhs },
                    (_, _) => Micro::Mul64 { ty, dst, lhs, rhs },
                }
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
/// One wrapping operation at one width, with nothing left to decide.
///
/// The result is what `int_op` would have returned: the low bits of the width, held
/// zero-extended, which is the same answer signed or unsigned because wrapping is.
macro_rules! wrap_arm {
    ($how:ident, $mask:expr, $ty:expr, $dst:expr, $lhs:expr, $rhs:expr,
     $file:expr, $spans:expr, $here:expr) => {{
        let (Some(a), Some(b)) = ($file.bits($lhs), $file.bits($rhs)) else {
            return Err(Stopped {
                fault: not_as_described("this says it works on whole numbers"),
                span: $spans[$here],
            });
        };
        $file.set_bits($dst, $ty, a.$how(b) & $mask);
    }};
}

macro_rules! int_arm {
    ($binop:expr, $ty:expr, $dst:expr, $lhs:expr, $rhs:expr,
     $file:expr, $spans:expr, $here:expr, $overflow:expr) => {{
        let (Some(a), Some(b)) = ($file.bits($lhs), $file.bits($rhs)) else {
            return Err(Stopped {
                fault: not_as_described("this says it works on whole numbers"),
                span: $spans[$here],
            });
        };
        let bits = int_op($binop, $ty, a, b, $overflow)
            .map_err(|fault| Stopped { fault, span: $spans[$here] })?;
        $file.set_bits($dst, $ty, bits);
    }};
}

/// One arrangement of a frame's registers.
///
/// Two live here. [`Boxed`] is a `Vec<Value>`, every register carrying its own tag —
/// the arrangement the tier hands across a join, so a tiered run uses it. [`Split`]
/// is the JIT's arrangement adopted: raw words for everything the opcode already
/// types, `Value` cells only for what is genuinely heterogeneous — so moving an `i64`
/// is a word, not a tagged sixteen-byte struct with drop glue. Both behind one loop,
/// monomorphised twice, which is also what lets one binary measure them against each
/// other; two builds of identical source differ by 8% on layout alone.
///
/// The `Option`s are the boxed tag check surviving as an interface: [`Split`] answers
/// `Some` unconditionally — the typing pass refused, at load, every chunk that could
/// make that wrong — and the compiler folds the branch out of that monomorphisation.
trait File {
    fn new(count: usize) -> Self;
    /// The stored bits of a number-shaped register.
    fn bits(&self, at: chunk::Reg) -> Option<u64>;
    /// Write a number a field at a time: a whole-value write is a sixteen-byte store
    /// that the next instruction's narrow load stalls on — 12x on Zen 3.
    fn set_bits(&mut self, at: chunk::Reg, ty: Ty, bits: u64);
    fn truth(&self, at: chunk::Reg) -> Option<bool>;
    /// The array a register names.
    fn handle(&self, at: chunk::Reg) -> Option<u32>;
    /// An index or a length, read the loose way `offset` always has.
    fn index(&self, at: chunk::Reg) -> i128;
    /// The index at sixty-four bits, which is every width this language has. Holding an
    /// unsigned value down at `i64::MAX` keeps the bounds comparison in one register —
    /// past that is past the end of any array there could be, and the fault path reads
    /// the real number again to report it.
    fn index_word(&self, at: chunk::Reg) -> i64;
    /// The whole value, for the paths that want one — built from the bits and the
    /// instruction's type where the register is raw.
    fn value(&self, at: chunk::Reg, ty: Ty) -> Value;
    /// A value that says what it is, put where it belongs.
    fn put(&mut self, at: chunk::Reg, value: Value);
    /// One element out of an array and into a register, each arrangement its own way —
    /// the word file straight from the stored bits, the boxed one through the `Value`
    /// it keeps anyway. `false` when there is no such element. `element` is the side
    /// the register lives on: a decimal is bits in the heap and a cell in the file,
    /// and routing by what the heap can give instead of where the register lives put
    /// d64 elements in the wrong place — found by seed 96 within the hour.
    fn load_element(&mut self, dst: chunk::Reg, handle: u32, index: usize, element: Ty) -> bool;
    /// One argument, caller's register to callee's, whatever it holds.
    fn argument(&mut self, at: usize, from: &Self, src: usize);
    /// Every root a collection must see.
    fn roots(&self) -> impl Iterator<Item = &Value>;
    /// The tier boundary speaks `Vec<Value>`, and only a boxed run reaches it.
    fn lend(&self) -> &Vec<Value>;
    fn into_values(self) -> Vec<Value>;
}

/// Every register a tagged [`Value`]. What the tier hands over, so tiered runs use it.
struct Boxed(Vec<Value>);

impl File for Boxed {
    fn new(count: usize) -> Self {
        // A register the checker has proved is written before it is read; the
        // placeholder is never observed by a program that got this far.
        Boxed(vec![Value::Bool(false); count])
    }
    fn bits(&self, at: chunk::Reg) -> Option<u64> {
        match &self.0[at as usize] {
            Value::Num { bits, .. } => Some(*bits),
            _ => None,
        }
    }
    fn set_bits(&mut self, at: chunk::Reg, ty: Ty, bits: u64) {
        if let Value::Num { ty: t, bits: b } = &mut self.0[at as usize] {
            *t = ty;
            *b = bits;
        } else {
            self.0[at as usize] = Value::Num { ty, bits };
        }
    }
    fn truth(&self, at: chunk::Reg) -> Option<bool> {
        truth(&self.0[at as usize])
    }
    fn handle(&self, at: chunk::Reg) -> Option<u32> {
        handle_of(&self.0[at as usize])
    }
    fn index(&self, at: chunk::Reg) -> i128 {
        self.0[at as usize].as_i128().unwrap_or(0)
    }
    fn index_word(&self, at: chunk::Reg) -> i64 {
        match &self.0[at as usize] {
            Value::Num { ty, bits } if ty.is_signed() => {
                let shift = 64 - ty.int_bits().unwrap_or(64);
                ((*bits << shift) as i64) >> shift
            }
            Value::Num { bits, .. } => (*bits).min(i64::MAX as u64) as i64,
            _ => 0,
        }
    }
    fn value(&self, at: chunk::Reg, _ty: Ty) -> Value {
        self.0[at as usize].clone()
    }
    fn put(&mut self, at: chunk::Reg, value: Value) {
        match (&mut self.0[at as usize], value) {
            (Value::Num { ty: t, bits: b }, Value::Num { ty, bits }) => {
                *t = ty;
                *b = bits;
            }
            (slot, value) => *slot = value,
        }
    }
    fn load_element(&mut self, dst: chunk::Reg, handle: u32, index: usize, _element: Ty) -> bool {
        match heap::read(handle, index) {
            Some(held) => {
                self.put(dst, held);
                true
            }
            None => false,
        }
    }
    fn argument(&mut self, at: usize, from: &Self, src: usize) {
        self.0[at] = from.0[src].clone();
    }
    fn roots(&self) -> impl Iterator<Item = &Value> {
        self.0.iter()
    }
    fn lend(&self) -> &Vec<Value> {
        &self.0
    }
    fn into_values(self) -> Vec<Value> {
        self.0
    }
}

/// Raw words beside cells: the JIT's split, adopted whole. The word is authoritative
/// for everything the opcode can type — integers, floats as their encodings, `bool`,
/// and array handles — and the cell beside it for the four wide things. A handle
/// lives in its word, where the loop that reads elements finds it without reaching
/// through a `Value`, and is *mirrored* into the cell beside it when written — the
/// JIT's own answer, one store at the sites that make or move an array, so the cells
/// stay the whole of the root set a collection walks. Celling the handle instead cost
/// the array loop 11% end to end, found by be against main where the in-binary
/// harness could not see it: the harness compares the two arrangements in this
/// binary, and the boxed one had drifted under the same refactor.
struct Split {
    raw: Vec<u64>,
    /// Grown to size on the first celled write, and empty until then: most frames hold
    /// nothing but words, and a call was paying a second allocation for a table of
    /// placeholders nothing would read. A typed chunk never reads a cell it has not
    /// written, so an empty table is only ever read by a root walk, which wants it
    /// empty.
    cells: Vec<Value>,
}

impl Split {
    /// A register's word, and a register's word written, without a bounds check.
    ///
    /// The promise this rests on is not made here and is not new: `serialize::check`
    /// says the VM "indexes registers, constants, text and instructions without
    /// checking, because the compiler never produces an index that is wrong", and then
    /// holds every register in every instruction of every routine against that
    /// routine's own register count before the chunk is handed back. A frame is made
    /// exactly `registers` wide from the same number. So the index is proved, once per
    /// instruction in the file, rather than once per instruction executed -- and the
    /// loop was still paying the second one.
    ///
    /// The *instruction* index is left checked, deliberately. `check` proves every jump
    /// target lands on a real instruction, and that a `Halt` exists somewhere, but not
    /// that control reaches it: a chunk ending in something other than a stop runs off
    /// the end of its own code. That is a panic today and would be worse than a panic
    /// here, so the fetch keeps its check until the chunk format demands a last word.
    #[inline(always)]
    fn word(&self, at: chunk::Reg) -> u64 {
        // SAFETY: see above -- `at` was proved below `raw.len()` when the chunk loaded.
        unsafe { *self.raw.get_unchecked(at as usize) }
    }
    #[inline(always)]
    fn set_word(&mut self, at: chunk::Reg, bits: u64) {
        // SAFETY: as `word`.
        unsafe { *self.raw.get_unchecked_mut(at as usize) = bits }
    }

    fn cells_now(&mut self) -> &mut Vec<Value> {
        if self.cells.len() < self.raw.len() {
            self.cells.resize(self.raw.len(), Value::Bool(false));
        }
        &mut self.cells
    }
}

/// Whether a register of this type lives in the cells. The JIT's line, exactly.
fn celled(ty: Ty) -> bool {
    matches!(ty, Ty::B128 | Ty::B256 | Ty::Str | Ty::Er) || ty.is_decimal()
}

impl File for Split {
    fn new(count: usize) -> Self {
        Split { raw: vec![0; count], cells: Vec::new() }
    }
    fn bits(&self, at: chunk::Reg) -> Option<u64> {
        Some(self.word(at))
    }
    fn set_bits(&mut self, at: chunk::Reg, _ty: Ty, bits: u64) {
        self.set_word(at, bits);
    }
    fn truth(&self, at: chunk::Reg) -> Option<bool> {
        Some(self.word(at) != 0)
    }
    fn handle(&self, at: chunk::Reg) -> Option<u32> {
        Some(self.word(at) as u32)
    }
    fn index(&self, at: chunk::Reg) -> i128 {
        i128::from(self.word(at))
    }
    fn index_word(&self, at: chunk::Reg) -> i64 {
        // An index is `u32` by the checker's own rule, and a chunk demonstrated it.
        self.word(at).min(i64::MAX as u64) as i64
    }
    fn value(&self, at: chunk::Reg, ty: Ty) -> Value {
        if celled(ty) {
            self.cells.get(at as usize).cloned().unwrap_or(Value::Bool(false))
        } else if ty == Ty::Bool {
            Value::Bool(self.word(at) != 0)
        } else {
            // Arrays come this way too: the word is the handle, and `Num` around it is
            // exactly the value the boxed file would have held.
            Value::Num { ty, bits: self.word(at) }
        }
    }
    fn put(&mut self, at: chunk::Reg, value: Value) {
        match value {
            // A handle's word is what the element loop reads; the mirror into the cell
            // is what the collector reads. Written here, at the sites that make or
            // move an array, never in the loop that reads one.
            Value::Num { ty: ty @ Ty::Array(_), bits } => {
                self.set_word(at, bits);
                self.cells_now()[at as usize] = Value::Num { ty, bits };
            }
            Value::Num { ty, bits } if !celled(ty) => self.set_word(at, bits),
            Value::Bool(answer) => self.set_word(at, u64::from(answer)),
            value => self.cells_now()[at as usize] = value,
        }
    }
    fn load_element(&mut self, dst: chunk::Reg, handle: u32, index: usize, element: Ty) -> bool {
        // Bits straight to the word — but only for a register that *lives* in its
        // word. A decimal is bits in the heap and a cell here, and the wide, text and
        // exact elements go the Value way regardless.
        if !celled(element)
            && let Some(bits) = heap::read_bits(handle, index)
        {
            // An element that is itself an array is a handle, and a handle written
            // anywhere must be mirrored where the collector looks — `put` knows.
            if matches!(element, Ty::Array(_)) {
                self.put(dst, Value::Num { ty: element, bits });
            } else {
                self.set_word(dst, bits);
            }
            return true;
        }
        match heap::read(handle, index) {
            Some(held) => {
                self.put(dst, held);
                true
            }
            None => false,
        }
    }
    fn argument(&mut self, at: usize, from: &Self, src: usize) {
        // Both sides, blindly: one of them is the argument and the other is nothing,
        // and copying nothing is cheaper than deciding. A caller with no cells at all
        // has no celled argument to hand over.
        self.raw[at] = from.raw[src];
        if let Some(value) = from.cells.get(src) {
            self.cells_now()[at] = value.clone();
        }
    }
    fn roots(&self) -> impl Iterator<Item = &Value> {
        self.cells.iter()
    }
    fn lend(&self) -> &Vec<Value> {
        unreachable!("only a tiered run lends frames, and a tiered run is boxed")
    }
    fn into_values(self) -> Vec<Value> {
        unreachable!("only a tiered run hands frames over, and a tiered run is boxed")
    }
}

/// What a build with no compiler in it should do about a chunk that wanted one.
///
/// `[run] mode` names an engine and `[run] engine` says how much the project meant it.
/// The default is still a preference — a program that would rather be fast runs slowly
/// rather than not at all — and a project whose program is unusable interpreted can now
/// say so instead of finding out from its users.
pub enum Without {
    /// Nothing was asked for that is not here.
    Fine,
    /// Asked for, not here, and the project said running anyway is acceptable.
    FallingBack(String),
    /// Asked for, not here, and the project said no.
    Refused(String),
}

/// Decide, and say it in words the person holding the wrong binary can act on.
///
/// `whose` names the thing that has no compiler in it, and `how` is the command that
/// builds one that has.
pub fn without_a_compiler(chunk: &Chunk, whose: &str, how: &str) -> Without {
    use luarust_core::value::Engine;
    let asked = match chunk.engine {
        Engine::Vm => return Without::Fine,
        Engine::Whole => "whole",
        Engine::Hot => "hot",
    };
    if chunk.insistence.may_fall_back() {
        return Without::FallingBack(format!(
            "this chunk asks for `[run] mode = \"{asked}\"` and {whose} has no JIT in it, \
             so it runs on the bytecode VM. Build one that has:\n\n    {how}\n"
        ));
    }
    Without::Refused(format!(
        "this chunk asks for `[run] mode = \"{asked}\"` and says `[run] engine = \
         \"required\"`, and {whose} has no JIT in it. It has not been run. Build one that \
         has:\n\n    {how}\n\nOr set `[run] engine = \"optional\"` in the project that \
         built it, and it will run on the VM instead."
    ))
}

/// Run a compiled chunk.
pub fn run(chunk: &Chunk, out: &mut impl Write) -> Result<(), Stopped> {
    engine::<Split>(chunk, out, None, true)
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
    match tier {
        // The tier boundary speaks Vec<Value>, so a tiered run keeps the boxed file.
        Some(tier) => engine::<Boxed>(chunk, out, Some(tier), true),
        None => engine::<Split>(chunk, out, None, true),
    }
}

/// [`run_with`], saying whether loop tails are fused, on the boxed file — the fusion
/// instrument, kept on one arrangement so it measures fusion and nothing else.
#[cfg(all(test, feature = "compile"))]
fn run_widened(
    chunk: &Chunk,
    out: &mut impl Write,
    tier: Option<&mut dyn Tier>,
    fuse: bool,
) -> Result<(), Stopped> {
    engine::<Boxed>(chunk, out, tier, fuse)
}

/// The machine itself, over either arrangement of the registers — one loop,
/// monomorphised per [`File`], which is what lets one binary hold and measure both.
fn engine<F: File>(
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
    let started = Instant::now();

    let mut frames: Vec<Frame<F>> = vec![Frame {
        routine: None,
        at: 0,
        file: F::new(chunk.registers),
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
        Some(tier) => (widen(&chunk.code, Some(&mut counters), fuse, chunk.overflow), tier.threshold().max(1)),
        None => (widen(&chunk.code, None, fuse, chunk.overflow), 0),
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
                        Some(_) => widen(&chunk.funcs[index].code, Some(&mut counters), fuse, chunk.overflow),
                        None => widen(&chunk.funcs[index].code, None, fuse, chunk.overflow),
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
            let file = &mut frames.last_mut().expect("a frame is always open").file;
            loop {
                // Checked, on purpose, and `Ends` in `serialize` is why it *could* be
                // otherwise: since a chunk has to end in a stop, `at` provably names a
                // real instruction and this could be `get_unchecked`. It was, and it
                // measured nothing -- -2.7% on one loop by best and +1.6% by median,
                // +0.4% on another -- because `at` is already in a register and the
                // compare predicts perfectly against a load that dominates it. Unsafe
                // that buys nothing is unsafe that costs something.
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
            Micro::AtOne { dst, array, from, shape } => {
                let Some(handle) = file.handle(array) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it indexes an array"),
                        span: spans[here],
                    });
                };
                let shape = shapes[shape as usize];
                let index = offset(shape.dims(), handle, file, from, 1)
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                if !file.load_element(dst, handle, index, shape.element) {
                    return Err(Stopped {
                        fault: out_of_range(index as i128 + 1, heap::length(handle) as i128),
                        span: spans[here],
                    });
                }
                // Past the copy's own `At`, which is left where it was and reached from
                // nowhere else -- `widen` checked that before fusing.
                at += 1;
            }
            Micro::At { dst, array, at, rank, shape } => {
                let Some(handle) = file.handle(array) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it indexes an array"),
                        span: spans[here],
                    });
                };
                // Found once. The dimensions and the element type are two fields of one
                // shape, and this was indexing the table for each of them.
                let shape = shapes[shape as usize];
                let index = offset(shape.dims(), handle, file, at, rank)
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                if !file.load_element(dst, handle, index, shape.element) {
                    return Err(Stopped {
                        fault: out_of_range(index as i128 + 1, heap::length(handle) as i128),
                        span: spans[here],
                    });
                }
            }
            Micro::Add8 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_add, 0xff, ty, dst, lhs, rhs, file, spans, here),
            Micro::Add16 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_add, 0xffff, ty, dst, lhs, rhs, file, spans, here),
            Micro::Add32 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_add, 0xffff_ffff, ty, dst, lhs, rhs, file, spans, here),
            Micro::Add64 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_add, u64::MAX, ty, dst, lhs, rhs, file, spans, here),
            Micro::Sub8 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_sub, 0xff, ty, dst, lhs, rhs, file, spans, here),
            Micro::Sub16 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_sub, 0xffff, ty, dst, lhs, rhs, file, spans, here),
            Micro::Sub32 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_sub, 0xffff_ffff, ty, dst, lhs, rhs, file, spans, here),
            Micro::Sub64 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_sub, u64::MAX, ty, dst, lhs, rhs, file, spans, here),
            Micro::Mul8 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_mul, 0xff, ty, dst, lhs, rhs, file, spans, here),
            Micro::Mul16 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_mul, 0xffff, ty, dst, lhs, rhs, file, spans, here),
            Micro::Mul32 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_mul, 0xffff_ffff, ty, dst, lhs, rhs, file, spans, here),
            Micro::Mul64 { ty, dst, lhs, rhs } =>
                wrap_arm!(wrapping_mul, u64::MAX, ty, dst, lhs, rhs, file, spans, here),

            Micro::Add { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Add, ty, dst, lhs, rhs, file, spans, here, chunk.overflow)
            }
            Micro::Sub { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Sub, ty, dst, lhs, rhs, file, spans, here, chunk.overflow)
            }
            Micro::Mul { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Mul, ty, dst, lhs, rhs, file, spans, here, chunk.overflow)
            }
            Micro::Div { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Div, ty, dst, lhs, rhs, file, spans, here, chunk.overflow)
            }
            Micro::Mod { ty, dst, lhs, rhs } => {
                int_arm!(BinOp::Mod, ty, dst, lhs, rhs, file, spans, here, chunk.overflow)
            }

            Micro::Tail { ty, counter, limit, step } => {
                let (Some(a), Some(b)) = (file.bits(counter), file.bits(limit)) else {
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
                        BinOp::Add, ty, counter, counter, step, file, spans, here,
                        chunk.overflow
                    );
                    // The jump back was left in the slot beside this one, unreachable
                    // except from here.
                    //
                    // `widen` is the only thing that makes a `Tail`, and it makes one
                    // only from the exact three instructions `compile` emits, with this
                    // jump the third -- so the tag is known before the fetch. Saying so
                    // costs the hottest instruction in the language a tag test and a
                    // panic path it can never take.
                    let Micro::Other(Op::Jump { target }) = code[at + 1] else {
                        // SAFETY: a fused tail keeps its jump beside it, by construction
                        // in `widen`, which is the only maker of `Micro::Tail`.
                        unsafe { std::hint::unreachable_unchecked() }
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
                let mut fresh = F::new(chunk.funcs[func as usize].registers);
                for n in 0..argc as usize {
                    fresh.argument(n, file, base as usize + n);
                }
                break Step::Called { func: func as usize, fresh, dst };
            }

            Op::Return { src, ty } => break Step::Returned(Some(file.value(src, ty))),

            Op::ReturnNothing => break Step::Returned(None),

            Op::Const { dst, konst } => {
                file.put(dst, chunk.consts[konst as usize].clone());
            }

            Op::Move { dst, src, ty } => {
                let held = file.value(src, ty);
                file.put(dst, held);
            }

            // Integers go straight to the arithmetic with their raw bits, since the
            // instruction already says what width they are. Everything else goes the long
            // way round, which for a b256 divide is nothing next to the divide.
            Op::Binary { op, ty, dst, lhs, rhs, .. } if ty.is_integer() => {
                int_arm!(op, ty, dst, lhs, rhs, file, spans, here, chunk.overflow)
            }

            Op::Binary { op, ty, dst, lhs, rhs, .. } => {
                let value = binary_op(
                    op,
                    &file.value(lhs, ty),
                    &file.value(rhs, ty),
                    chunk.overflow,
                )
                .map_err(|fault| Stopped { fault, span: spans[here] })?;
                file.put(dst, value);
            }

            Op::Compare { op, operands, dst, lhs, rhs } if operands.is_integer() => {
                let (Some(a), Some(b)) = (file.bits(lhs), file.bits(rhs)) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it compares whole numbers"),
                        span: spans[here],
                    });
                };
                file.put(dst, Value::Bool(holds(op, int_compare(operands, a, b))));
            }

            Op::Compare { op, operands, dst, lhs, rhs } => {
                let ordering = compare(&file.value(lhs, operands), &file.value(rhs, operands));
                file.put(dst, Value::Bool(holds(op, ordering)));
            }

            Op::Neg { dst, src, ty } => {
                let value = negate(&file.value(src, ty), chunk.overflow)
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                file.put(dst, value);
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
                file.put(dst, Value::float(ty, bits));
            }

            Op::PrintText { text } => {
                let _ = out.write_all(chunk.texts[text as usize].as_bytes());
                let _ = out.flush();
            }

            Op::PrintValue { src, ty } => {
                let _ = out.write_all(file.value(src, ty).to_string().as_bytes());
                let _ = out.flush();
            }

            Op::NewArray { dst, items, count, ty } => {
                let of = ty.array().expect("a new array has an array type");
                let held: Vec<Value> = (0..count)
                    .map(|n| file.value(items + n, of.element))
                    .collect();
                file.put(dst, heap::handle(ty, heap::of(of.element, &held)));
                if heap::wants_collecting() {
                    break Step::Collecting;
                }
            }

            Op::Filled { dst, length, value, ty } => {
                let of = ty.array().expect("a filled array has an array type");
                let count = file.index(length);
                if count < 0 {
                    return Err(Stopped { fault: fewer_than_none(), span: spans[here] });
                }
                let fill = file.value(value, of.element);
                file.put(dst, heap::handle(ty, heap::make(of.element, count as usize, &fill)));
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
                let Some(handle) = file.handle(array) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it indexes an array"),
                        span: spans[here],
                    });
                };
                let shape = ty.array().expect("only an array is indexed");
                let index = offset(shape.dims(), handle, file, at, rank)
                    .map_err(|fault| Stopped { fault, span: spans[here] })?;
                let held = file.value(value, shape.element);
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
                let Some(handle) = file.handle(array) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it indexes an array"),
                        span: spans[here],
                    });
                };
                file.set_bits(dst, ty, heap::length(handle) as u64);
            }

            Op::Not { dst, src } => {
                let Some(answer) = file.truth(src) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it negates a truth"),
                        span: spans[here],
                    });
                };
                file.put(dst, Value::Bool(!answer));
            }

            Op::JumpIfFalse { cond, target } => {
                let Some(answer) = file.truth(cond) else {
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
                let Some(answer) = file.truth(cond) else {
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
                let (Some(a), Some(b)) = (file.bits(lhs), file.bits(rhs)) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it compares whole numbers"),
                        span: spans[here],
                    });
                };
                if int_compare(ty, a, b) == Comparison::Greater {
                    at = target as usize;
                }
            }

            Op::JumpIfEqual { lhs, rhs, ty, target } if ty.is_integer() => {
                let (Some(a), Some(b)) = (file.bits(lhs), file.bits(rhs)) else {
                    return Err(Stopped {
                        fault: not_as_described("this says it compares whole numbers"),
                        span: spans[here],
                    });
                };
                if a == b {
                    at = target as usize;
                }
            }

            Op::JumpIfGreater { lhs, rhs, ty, target } => {
                if compare(&file.value(lhs, ty), &file.value(rhs, ty))
                    == Comparison::Greater
                {
                    at = target as usize;
                }
            }

            Op::JumpIfEqual { lhs, rhs, ty, target } => {
                if compare(&file.value(lhs, ty), &file.value(rhs, ty))
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
                        frames.iter().map(|frame| frame.file.lend()).collect();
                    let answer = t.call(chunk, func, &open, fresh.into_values(), started, out)?;
                    if let Some(answer) = answer {
                        let caller = frames.last_mut().expect("something called it");
                        caller.file.put(dst, answer);
                    }
                    continue 'activation;
                }
                frames.push(Frame { routine: Some(func), at: 0, file: fresh, dst });
            }
            Step::Returned(answer) => {
                let finished = frames.pop().expect("a frame is always open");
                if let Some(answer) = answer {
                    let caller = frames.last_mut().expect("something called it");
                    caller.file.put(finished.dst, answer);
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
                    frames.iter().map(|frame| frame.file.lend().clone()).collect();
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
                            caller.file.put(finished.dst, answer);
                        }
                    }
                }
            }
            Step::Collecting => {
                frames.last_mut().expect("a frame is always open").at = at;
                // Every root of every frame: the whole file where values carry tags, the
                // cells alone where the words cannot be handles.
                heap::collect(frames.iter().flat_map(|frame| frame.file.roots()));
            }
        }
        continue 'activation;
    }
}

/// What ended a run of instructions: something that changes which frame is running.
enum Step<F> {
    Called { func: usize, fresh: F, dst: u16 },
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
fn offset<F: File>(
    dims: &[u32],
    handle: u32,
    file: &F,
    at: u16,
    rank: u8,
) -> Result<usize, Box<Fault>> {
    // `Shape::dims` hands back `dims[..rank]` and a rank is never zero except on an array
    // that grows, where the slice is empty. So the trailing zeroes this used to scan for,
    // on every element read, were never in the slice it was scanning.
    debug_assert!(!dims.contains(&0), "a shape's dimensions are already trimmed");

    // One dimension, which is most arrays, without the loop that carries the general
    // case: no accumulator to multiply into, no `get` on a slice of one, and the bound
    // read straight out rather than through an `Option` per element.
    if rank == 1 {
        let held: i64 = file.index_word(at);
        let past = dims.first().map(|past| i64::from(*past));
        if held < 1 || past.is_some_and(|past| held > past) {
            let reported = file.index(at);
            return Err(out_of_range(
                reported,
                past.map_or_else(|| heap::length(handle) as i128, i128::from),
            ));
        }
        return Ok(held as usize - 1);
    }

    let mut flat = 0usize;

    for place in 0..rank as usize {
        // The index at sixty-four bits, which is every width this language has. It was
        // `as_i128`, and widening cost a match, a pair of 128-bit shifts and 128-bit
        // comparisons on every element read, to hold a number that never needed the room.
        // The same mistake as `Division::apply` made, in a hotter place.
        let held: i64 = file.index_word(at + place as u16);
        // A shaped array knows its bounds without asking. One that grows does not -- and
        // `heap::read` is about to check the index against the real length anyway, so
        // asking here as well meant two trips through the heap for every element read,
        // the second of them computing a number only the error path ever looks at. Too
        // small an index is still caught here, because that one nothing downstream sees.
        let past = (!dims.is_empty()).then(|| i64::from(dims[place]));
        if held < 1 || past.is_some_and(|past| held > past) {
            let reported = file.index(at + place as u16);
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

    /// The other instrument: the two register files, timed from the one binary.
    #[cfg(feature = "compile")]
    #[test]
    #[ignore = "a measurement, run by hand with --nocapture"]
    fn files_split_against_boxed() {
        let add = "var.local.mut.i64 ['sum'] = [|0|];\n\
                   loop.temp.range.i64 ['i'] = [|1|, |30000000|] {\n\
                   set ['sum'] = [math { 'sum' + 'i' }];\n\
                   }\nprint['sum' \\n];\n";
        let array = "var.local.mut.array.i64 ['xs'] = [filled[|64|, |7|]];\n\
                     var.local.mut.i64 ['sum'] = [|0|];\n\
                     loop.temp.range.ui32 ['i'] = [|1|, |30000000|] {\n\
                     set ['sum'] = [math { 'sum' + 'xs'[math { (('i' - 1) mod 64) + 1 }] }];\n\
                     }\nprint['sum' \\n];\n";
        let calls = "fn.local.i64 ['leaf'] [i64 'n'] {\n\
                     return math { ('n' * 3) mod 1000000007 };\n\
                     }\n\
                     var.local.mut.i64 ['sum'] = [|0|];\n\
                     loop.temp.range.i64 ['i'] = [|1|, |3000000|] {\n\
                     set ['sum'] = [math { ('sum' + leaf['i']) mod 1000000007 }];\n\
                     }\nprint['sum' \\n];\n";
        for (name, source, iterations) in
            [("add", add, 30_000_000u64), ("array", array, 30_000_000), ("calls", calls, 3_000_000)]
        {
            let chunk = chunk_of(source);
            let (mut split, mut boxed) = (u128::MAX, u128::MAX);
            for _ in 0..6 {
                let mut out = Vec::new();
                let t0 = std::time::Instant::now();
                crate::engine::<crate::Split>(&chunk, &mut out, None, true).expect("it runs");
                split = split.min(t0.elapsed().as_nanos());
                let mut out = Vec::new();
                let t0 = std::time::Instant::now();
                crate::engine::<crate::Boxed>(&chunk, &mut out, None, true).expect("it runs");
                boxed = boxed.min(t0.elapsed().as_nanos());
            }
            println!(
                "{name}: split {} ms, boxed {} ms ({:.2} against {:.2} ns an iteration)",
                split / 1_000_000,
                boxed / 1_000_000,
                split as f64 / iterations as f64,
                boxed as f64 / iterations as f64,
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

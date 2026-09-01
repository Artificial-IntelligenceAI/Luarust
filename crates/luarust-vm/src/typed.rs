//! Whether a chunk's instructions agree about what its registers hold.
//!
//! The checker proves this about programs it compiles — "an instruction's type is the
//! type of the values it works on" — but a chunk can arrive from anywhere, and until
//! now the claim was tested one instruction at a time, as it ran: the tag in the
//! register caught a lie the moment it executed (R0016). An unboxed register file has
//! no tag to catch anything with, so the whole claim is established here instead, once,
//! when the chunk is read — and a chunk that types inconsistently is refused as
//! [`Broken`], the way every other malformed chunk is.
//!
//! Stronger than the tag it replaces, in one honest sense: a lie on a path that never
//! runs was invisible to the tag and is refused here. And what it defends is the fault
//! contract, not the process — even with no check at all, a lying chunk against raw
//! words computes wrong answers rather than anything unsafe, since cells always hold
//! real values and the heap checks its own handles.
//!
//! The walk is a fixed point over the jump graph. Each register at each instruction is
//! [`Slot::Unset`], holds one known type, or is [`Slot::Clash`] — written as two
//! different types on two ways here. A clash is not itself refused: the compiler reuses
//! a dead variable's register freely, and a dead register may disagree with itself all
//! it likes. Only *reading* one is a lie.

use crate::chunk::{Chunk, Op, Routine};
use crate::serialize::Broken;
use luarust_core::Ty;
use luarust_core::value::Value;

/// What one register is known to hold at one point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Slot {
    /// Never written on any way here. The checker proves this is never read; a chunk
    /// has to demonstrate it.
    Unset,
    Is(Ty),
    /// Written as different types on different ways here. Fine to hold, a lie to read.
    Clash,
}

fn join(a: Slot, b: Slot) -> Slot {
    if a == b { a } else { Slot::Clash }
}

/// Refuse any instruction in the chunk that disagrees with its registers.
pub fn well_typed(chunk: &Chunk) -> Result<(), Broken> {
    walk(chunk, &chunk.code, None)?;
    for routine in &chunk.funcs {
        walk(chunk, &routine.code, Some(routine))?;
    }
    Ok(())
}

/// One body, to its fixed point.
fn walk(chunk: &Chunk, code: &[Op], routine: Option<&Routine>) -> Result<(), Broken> {
    let registers = match routine {
        Some(routine) => routine.registers,
        None => chunk.registers,
    };
    let mut entry = Slots { held: vec![Slot::Unset; registers] };
    if let Some(routine) = routine {
        for (register, ty) in routine.params.iter().enumerate() {
            entry.held[register] = Slot::Is(*ty);
        }
    }

    // The state known at each instruction's door, and which doors want another look.
    let mut at_door: Vec<Option<Slots>> = vec![None; code.len()];
    if code.is_empty() {
        return Ok(());
    }
    at_door[0] = Some(entry);
    let mut asking = vec![0usize];

    while let Some(here) = asking.pop() {
        let mut state = at_door[here].clone().expect("only asked about doors with state");
        let op = code[here];
        step(chunk, op, here, &mut state)?;
        for gone in goes_to(op, here, code.len()) {
            let merged = match &at_door[gone] {
                None => state.clone(),
                Some(seen) => {
                    let merged = Slots {
                        held: seen
                            .held
                            .iter()
                            .zip(&state.held)
                            .map(|(a, b)| join(*a, *b))
                            .collect(),
                    };
                    if merged == *seen {
                        continue;
                    }
                    merged
                }
            };
            at_door[gone] = Some(merged);
            asking.push(gone);
        }
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq)]
struct Slots {
    held: Vec<Slot>,
}

impl Slots {
    /// The instruction says this register holds a `ty`. Make it demonstrate that.
    fn reads(&self, register: u16, ty: Ty, here: usize) -> Result<(), Broken> {
        match self.held.get(register as usize) {
            Some(Slot::Is(held)) if *held == ty => Ok(()),
            _ => Err(Broken::Mistyped { at: here as u64, register: u64::from(register) }),
        }
    }

    fn writes(&mut self, register: u16, ty: Ty, here: usize) -> Result<(), Broken> {
        match self.held.get_mut(register as usize) {
            Some(slot) => {
                *slot = Slot::Is(ty);
                Ok(())
            }
            None => Err(Broken::Mistyped { at: here as u64, register: u64::from(register) }),
        }
    }
}

/// What one instruction reads, then what it writes.
fn step(chunk: &Chunk, op: Op, here: usize, state: &mut Slots) -> Result<(), Broken> {
    match op {
        Op::Const { dst, konst } => {
            // The pool is already index-checked at read; what matters here is the type.
            let ty = chunk
                .consts
                .get(konst as usize)
                .map(Value::ty)
                .ok_or(Broken::Mistyped { at: here as u64, register: u64::from(dst) })?;
            state.writes(dst, ty, here)?;
        }
        Op::Move { dst, src, ty } => {
            state.reads(src, ty, here)?;
            state.writes(dst, ty, here)?;
        }
        Op::Binary { ty, dst, lhs, rhs, .. } => {
            state.reads(lhs, ty, here)?;
            state.reads(rhs, ty, here)?;
            state.writes(dst, ty, here)?;
        }
        Op::Neg { dst, src, ty } => {
            state.reads(src, ty, here)?;
            state.writes(dst, ty, here)?;
        }
        Op::Compare { operands, dst, lhs, rhs, .. } => {
            state.reads(lhs, operands, here)?;
            state.reads(rhs, operands, here)?;
            state.writes(dst, Ty::Bool, here)?;
        }
        Op::TimeNow { dst, ty } => state.writes(dst, ty, here)?,
        Op::PrintText { .. } => {}
        Op::PrintValue { src, ty } => state.reads(src, ty, here)?,
        Op::Call { func, base, argc, dst } => {
            let routine = chunk
                .funcs
                .get(func as usize)
                .ok_or(Broken::Mistyped { at: here as u64, register: u64::from(base) })?;
            if routine.params.len() != argc as usize {
                return Err(Broken::Mistyped { at: here as u64, register: u64::from(base) });
            }
            for (n, ty) in routine.params.iter().enumerate() {
                state.reads(base + n as u16, *ty, here)?;
            }
            if let Some(ty) = routine.returns {
                state.writes(dst, ty, here)?;
            }
        }
        Op::Return { src, ty } => state.reads(src, ty, here)?,
        Op::ReturnNothing | Op::Halt | Op::Jump { .. } => {}
        Op::NewArray { dst, items, count, ty } => {
            let of = ty
                .array()
                .ok_or(Broken::Mistyped { at: here as u64, register: u64::from(dst) })?;
            for n in 0..count {
                state.reads(items + n, of.element, here)?;
            }
            state.writes(dst, ty, here)?;
        }
        Op::Filled { dst, length, value, ty } => {
            let of = ty
                .array()
                .ok_or(Broken::Mistyped { at: here as u64, register: u64::from(dst) })?;
            state.reads(length, Ty::U32, here)?;
            state.reads(value, of.element, here)?;
            state.writes(dst, ty, here)?;
        }
        // `ty` on the indexing instructions is the *array's* type, shape and all; what
        // an element read answers, and a store consumes, is that shape's element.
        Op::At { dst, array, at, rank, ty } => {
            let of = indexed(array, at, rank, ty, here, state)?;
            state.writes(dst, of, here)?;
        }
        Op::StoreAt { array, at, rank, value, ty } => {
            let of = indexed(array, at, rank, ty, here, state)?;
            state.reads(value, of, here)?;
        }
        Op::Count { dst, array, ty } => {
            array_of(state, array, here)?;
            state.writes(dst, ty, here)?;
        }
        Op::Not { dst, src } => {
            state.reads(src, Ty::Bool, here)?;
            state.writes(dst, Ty::Bool, here)?;
        }
        Op::JumpIfFalse { cond, .. } | Op::JumpIfTrue { cond, .. } => {
            state.reads(cond, Ty::Bool, here)?;
        }
        Op::JumpIfGreater { lhs, rhs, ty, .. } | Op::JumpIfEqual { lhs, rhs, ty, .. } => {
            state.reads(lhs, ty, here)?;
            state.reads(rhs, ty, here)?;
        }
    }
    Ok(())
}

/// The element type of the array being indexed, with the register held to the array
/// type the instruction claims and each index read as `u32`.
fn indexed(
    array: u16,
    at: u16,
    rank: u8,
    ty: Ty,
    here: usize,
    state: &Slots,
) -> Result<Ty, Broken> {
    state.reads(array, ty, here)?;
    let of = ty
        .array()
        .map(|shape| shape.element)
        .ok_or(Broken::Mistyped { at: here as u64, register: u64::from(array) })?;
    for n in 0..rank {
        state.reads(at + u16::from(n), Ty::U32, here)?;
    }
    Ok(of)
}

/// The element type of whatever array a register holds.
fn array_of(state: &Slots, array: u16, here: usize) -> Result<Ty, Broken> {
    match state.held.get(array as usize) {
        Some(Slot::Is(held)) => held
            .array()
            .map(|of| of.element)
            .ok_or(Broken::Mistyped { at: here as u64, register: u64::from(array) }),
        _ => Err(Broken::Mistyped { at: here as u64, register: u64::from(array) }),
    }
}

/// Where an instruction can go next.
fn goes_to(op: Op, here: usize, len: usize) -> Vec<usize> {
    let next = here + 1;
    let step = match op {
        Op::Jump { target } => return vec![target as usize],
        Op::Return { .. } | Op::ReturnNothing | Op::Halt => return Vec::new(),
        Op::JumpIfFalse { target, .. }
        | Op::JumpIfTrue { target, .. }
        | Op::JumpIfGreater { target, .. }
        | Op::JumpIfEqual { target, .. } => {
            if next < len {
                return vec![target as usize, next];
            }
            return vec![target as usize];
        }
        _ => next,
    };
    if step < len { vec![step] } else { Vec::new() }
}

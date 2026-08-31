//! What compiled code calls back into.
//!
//! Machine code can add and divide on its own. It cannot print, read a clock, or work out
//! what `b256` division comes to — so for those it calls these, which are the same
//! functions the interpreter and the VM use. That is not a shortcut: it is what keeps the
//! three paths giving the same answers instead of nearly the same ones.

use luarust_check::value::{Overflow, Value, binary_op, compare, format_of, negate};
use luarust_num::binary::{self, Comparison, Round};
use luarust_parse::ast::{BinOp, Ty};
use std::cell::RefCell;
use std::time::Instant;

thread_local! {
    /// Values compiled code cannot hold.
    ///
    /// A `b256` is sixty-four bytes of significand and a `str` is a string; neither fits
    /// in a register, and the machine has no instructions for either. So they stay on this
    /// side, in numbered cells, and compiled code carries the *number* — which is known
    /// when it is compiled, so it costs nothing at all to carry.
    ///
    /// Everything done to them is a call. That is not a compromise: the arithmetic was
    /// always going to be a call, because `b128` and `b256` have no hardware anywhere and
    /// their answers have to come from the same place the other two execution paths get
    /// theirs.
    /// One frame of cells per call in progress. The compiled code always names a cell by
    /// its offset inside the frame it is running in, which is what makes a function that
    /// calls itself safe: each call gets its own row and nobody overwrites a caller's.
    static FRAMES: RefCell<Vec<Vec<Value>>> = const { RefCell::new(Vec::new()) };
    /// Cells nothing writes to, shared by every frame. Their numbers carry [`CONSTANT`].
    static CONSTANTS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    /// What each function's frame starts out holding, so entering one is a clone.
    static TEMPLATES: RefCell<Vec<Vec<Value>>> = const { RefCell::new(Vec::new()) };
    /// Celled arguments, staged by the caller in its own frame and taken by the callee in
    /// its new one. Going through here is what saves compiled code from ever having to
    /// name a cell in a frame other than its own.
    static PENDING: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    /// A celled answer, on its way from the frame that made it to the frame that asked.
    static ANSWER: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// The index that was out of range, and how many there were, so the fault can say so.
    /// Compiled code knows both and the fault code carries neither.
    static REACHED: RefCell<(i128, i128)> = const { RefCell::new((0, 0)) };
    /// What the running program has printed. Collected rather than streamed, because a
    /// callback cannot easily be handed the caller's writer.
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    /// When the program started, for the clock.
    static STARTED: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

/// Set on a cell number that means a constant rather than an offset into a frame.
pub const CONSTANT: u64 = 1 << 63;

/// Begin a run: forget the last one's arrays and output, restart the clock, and lay out
/// the cells.
///
/// The heap goes with the rest. The interpreter and the VM each forget it when they
/// start, and a path that did not would inherit whatever the last one left -- which is
/// exactly what happens in `luarust fuzz`, where all three run in the one process.
pub fn begin(constants: Vec<Value>, main_frame: Vec<Value>, templates: Vec<Vec<Value>>) {
    luarust_core::heap::clear();
    OUTPUT.with(|out| out.borrow_mut().clear());
    STARTED.with(|at| *at.borrow_mut() = Some(Instant::now()));
    CONSTANTS.with(|table| *table.borrow_mut() = constants);
    TEMPLATES.with(|table| *table.borrow_mut() = templates);
    PENDING.with(|queue| queue.borrow_mut().clear());
    ANSWER.with(|slot| *slot.borrow_mut() = None);
    FRAMES.with(|frames| *frames.borrow_mut() = vec![main_frame]);
}

fn cell(index: u64) -> Value {
    if index & CONSTANT != 0 {
        return CONSTANTS.with(|table| table.borrow()[(index & !CONSTANT) as usize].clone());
    }
    FRAMES.with(|frames| {
        frames.borrow().last().expect("a frame is always open")[index as usize].clone()
    })
}

fn put(index: u64, value: Value) {
    debug_assert!(index & CONSTANT == 0, "a constant cell is never written to");
    FRAMES.with(|frames| {
        frames.borrow_mut().last_mut().expect("a frame is always open")[index as usize] = value;
    });
}

/// Stage one celled argument, read from the caller's frame.
pub extern "C" fn cell_stage(index: u64) {
    let value = cell(index);
    PENDING.with(|queue| queue.borrow_mut().push(value));
}

/// Enter a function: a fresh frame, with the staged arguments still waiting.
///
/// They wait rather than being placed, because where they belong is a matter for the
/// callee: a celled parameter lives in the cell of whichever register it was given, and
/// the caller has no way to know which that is.
pub extern "C" fn cells_enter(routine: u64) {
    let frame = TEMPLATES.with(|table| table.borrow()[routine as usize].clone());
    FRAMES.with(|frames| frames.borrow_mut().push(frame));
}

/// Take the next staged argument into a cell of the frame just entered.
pub extern "C" fn cell_unstage(dst: u64) {
    let value = PENDING.with(|queue| {
        let mut queue = queue.borrow_mut();
        if queue.is_empty() { None } else { Some(queue.remove(0)) }
    });
    if let Some(value) = value {
        put(dst, value);
    }
}

/// Leave a function, keeping one cell's value for whoever asked.
pub extern "C" fn cells_leave_with(index: u64) {
    let value = cell(index);
    ANSWER.with(|slot| *slot.borrow_mut() = Some(value));
    FRAMES.with(|frames| {
        frames.borrow_mut().pop();
    });
}

/// Leave a function that had nothing to give back.
pub extern "C" fn cells_leave() {
    FRAMES.with(|frames| {
        frames.borrow_mut().pop();
    });
}

/// Take the answer a call left, into a cell of the frame that asked for it.
pub extern "C" fn cell_take_answer(dst: u64) {
    let value = ANSWER.with(|slot| slot.borrow_mut().take()).expect("a call left an answer");
    put(dst, value);
}

/// How deep the frames go, so compiled code can stop where the other two paths stop.
pub extern "C" fn call_depth() -> u64 {
    FRAMES.with(|frames| frames.borrow().len() as u64)
}

/// Copy one cell into another.
pub extern "C" fn cell_move(dst: u64, src: u64) {
    let value = cell(src);
    put(dst, value);
}

/// Arithmetic on two cells, into a third. Reading happens before writing, so the
/// destination may be one of the sources.
pub extern "C" fn cell_binary(op: u32, dst: u64, a: u64, b: u64, trapping: u32) -> i64 {
    let overflow = if trapping == 0 { Overflow::Wrap } else { Overflow::Trap };
    let (x, y) = (cell(a), cell(b));
    match binary_op(unop(op), &x, &y, overflow) {
        Ok(value) => {
            put(dst, value);
            OK
        }
        Err(fault) => fault_code(&fault),
    }
}

/// Negate a cell into another.
pub extern "C" fn cell_neg(dst: u64, src: u64, trapping: u32) -> i64 {
    let overflow = if trapping == 0 { Overflow::Wrap } else { Overflow::Trap };
    match negate(&cell(src), overflow) {
        Ok(value) => {
            put(dst, value);
            OK
        }
        Err(fault) => fault_code(&fault),
    }
}

/// How two cells order, the way every other execution path orders them.
pub extern "C" fn cell_compare(a: u64, b: u64) -> i32 {
    match compare(&cell(a), &cell(b)) {
        Comparison::Less => 0,
        Comparison::Equal => 1,
        Comparison::Greater => 2,
        Comparison::Unordered => 3,
    }
}

/// Print a cell, the way every other execution path prints one.
pub extern "C" fn print_cell(index: u64) {
    let text = cell(index).to_string();
    OUTPUT.with(|out| out.borrow_mut().extend_from_slice(text.as_bytes()));
}

/// Read the clock into a cell, in whichever float format it was asked for.
pub extern "C" fn cell_time_now(dst: u64, tag: u32) {
    let ty = untag(tag);
    let seconds = STARTED.with(|at| {
        at.borrow().map(|started| started.elapsed().as_secs_f64()).unwrap_or(0.0)
    });
    let fmt = format_of(ty).expect("the clock is read as a float");
    let bits = binary::from_decimal::<8>(fmt, Round::TiesToEven, &format!("{seconds:.9}"))
        .expect("nine decimal places is a number");
    put(dst, Value::float(ty, bits));
}

fn fault_code(fault: &luarust_check::value::Fault) -> i64 {
    match fault.code {
        "R0002" => DIVIDE_BY_ZERO,
        "R0003" => REMAINDER_BY_ZERO,
        "R0005" => DOES_NOT_FIT,
        "R0012" => FRACTIONAL_POWER,
        "R0013" => POWER_TOO_LARGE,
        "R0015" => OUT_OF_RANGE,
        _ => OTHER,
    }
}

/// Everything the run printed.
pub fn taken() -> Vec<u8> {
    OUTPUT.with(|out| std::mem::take(&mut *out.borrow_mut()))
}

/// Each type, as a number compiled code can carry.
pub fn tag_of(ty: Ty) -> u32 {
    match ty {
        Ty::I8 => 0,
        Ty::I16 => 1,
        Ty::I32 => 2,
        Ty::I64 => 3,
        Ty::U8 => 4,
        Ty::U16 => 5,
        Ty::U32 => 6,
        Ty::U64 => 7,
        Ty::B16 => 8,
        Ty::B32 => 9,
        Ty::B64 => 10,
        Ty::Bool => 19,
        Ty::Str => 20,
        _ => u32::MAX,
    }
}

fn untag(tag: u32) -> Ty {
    match tag {
        19 => Ty::Bool,
        20 => Ty::Str,
        0 => Ty::I8,
        1 => Ty::I16,
        2 => Ty::I32,
        3 => Ty::I64,
        4 => Ty::U8,
        5 => Ty::U16,
        6 => Ty::U32,
        7 => Ty::U64,
        8 => Ty::B16,
        9 => Ty::B32,
        _ => Ty::B64,
    }
}

/// Rebuild a value from the bits compiled code was holding it in.
fn value_of(tag: u32, bits: u64) -> Value {
    match untag(tag) {
        Ty::Bool => Value::Bool(bits != 0),
        ty => Value::Num { ty, bits },
    }
}

/// Which operation, as a number compiled code can carry.
pub fn op_tag(op: BinOp) -> u32 {
    match op {
        BinOp::Add => 0,
        BinOp::Sub => 1,
        BinOp::Mul => 2,
        BinOp::Div => 3,
        BinOp::Mod => 4,
        BinOp::Pow => 5,
    }
}

fn unop(tag: u32) -> BinOp {
    match tag {
        0 => BinOp::Add,
        1 => BinOp::Sub,
        2 => BinOp::Mul,
        3 => BinOp::Div,
        4 => BinOp::Mod,
        _ => BinOp::Pow,
    }
}

/// Fault codes, as compiled code returns them.
pub const OK: i64 = 0;
pub const DIVIDE_BY_ZERO: i64 = 1;
pub const REMAINDER_BY_ZERO: i64 = 2;
pub const DOES_NOT_FIT: i64 = 3;
pub const OTHER: i64 = 4;
pub const TOO_DEEP: i64 = 5;
pub const FRACTIONAL_POWER: i64 = 6;
pub const POWER_TOO_LARGE: i64 = 7;
pub const OUT_OF_RANGE: i64 = 8;

/// Print a piece of text.
///
/// # Safety
/// `ptr` must point at `len` readable bytes of UTF-8, which is what the compiler emits.
pub unsafe extern "C" fn print_text(ptr: *const u8, len: u64) {
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    OUTPUT.with(|out| out.borrow_mut().extend_from_slice(bytes));
}

/// Print a number, the way every other execution path prints one.
pub extern "C" fn print_value(bits: u64, tag: u32) {
    let text = value_of(tag, bits).to_string();
    OUTPUT.with(|out| out.borrow_mut().extend_from_slice(text.as_bytes()));
}

/// Seconds since the program began, in whichever float format was asked for.
///
/// Takes the type rather than always answering in `b64`, because a `b32` variable holding
/// a `b64`'s bits is a value that is not what it says it is.
pub extern "C" fn time_now(tag: u32) -> u64 {
    let seconds = STARTED.with(|at| {
        at.borrow().map(|started| started.elapsed().as_secs_f64()).unwrap_or(0.0)
    });
    let fmt = format_of(untag(tag)).expect("the clock is read as a float");
    binary::from_decimal::<8>(fmt, Round::TiesToEven, &format!("{seconds:.9}"))
        .expect("nine decimal places is a number")
        .low64()
}

/// How two values order, the way every other execution path orders them.
///
/// Compiled code does this itself for the types where an integer or a float comparison is
/// exactly right. `b16` is not one of those: its values are sign-and-magnitude in sixteen
/// bits, which is neither.
pub extern "C" fn compare_values(tag: u32, a: u64, b: u64) -> i32 {
    match compare(&value_of(tag, a), &value_of(tag, b)) {
        Comparison::Less => 0,
        Comparison::Equal => 1,
        Comparison::Greater => 2,
        Comparison::Unordered => 3,
    }
}

/// What [`compare_values`] answers.
pub const LESS: u64 = 0;
pub const EQUAL: u64 = 1;
pub const GREATER: u64 = 2;

/// Anything compiled code did not want to do itself.
///
/// # Safety
/// `out` must point at a writable `u64`.
pub unsafe extern "C" fn fallback(
    op: u32,
    tag: u32,
    a: u64,
    b: u64,
    trapping: u32,
    out: *mut u64,
) -> i64 {
    let overflow = if trapping == 0 { Overflow::Wrap } else { Overflow::Trap };
    let result = binary_op(unop(op), &value_of(tag, a), &value_of(tag, b), overflow);
    match result {
        Ok(Value::Num { bits, .. }) => {
            unsafe { *out = bits };
            OK
        }
        Ok(_) => OTHER,
        Err(fault) => fault_code(&fault),
    }
}


// ---- arrays ---------------------------------------------------------------------

/// Where an array's elements are. Compiled code takes this and does its own arithmetic.
pub extern "C" fn array_base(handle: u64) -> *mut u8 {
    luarust_core::heap::base_of(handle as u32).0
}

/// How many elements an array holds.
pub extern "C" fn array_len(handle: u64) -> u64 {
    luarust_core::heap::length(handle as u32) as u64
}

/// A new array of `count` elements taken from cells, one after another from `first`.
/// Sweep, if the heap has asked to be, before making one more array.
///
/// Here rather than after, so the array about to be made is never a candidate. Every
/// frame's cells are the roots: an array handle is written to its cell as well as its
/// register precisely so that this can see it.
///
/// Anything a register holds that is not a handle is looked at and passed over, and a
/// cell whose register has since been reused for something else keeps its old array
/// alive. That is conservative -- it holds on to something dead, never frees something
/// live -- and it is the trade a collector for compiled code without stack maps makes.
fn sweep_if_asked() {
    if !luarust_core::heap::wants_collecting() {
        return;
    }
    FRAMES.with(|frames| {
        let frames = frames.borrow();
        luarust_core::heap::collect(frames.iter().flatten());
    });
}

pub extern "C" fn array_new(element: u32, first: u64, count: u64) -> u64 {
    sweep_if_asked();
    let element = Ty::from_tag(element as u8).expect("an element tag came from a type");
    let held: Vec<Value> = (0..count).map(|n| cell(first + n)).collect();
    u64::from(luarust_core::heap::of(element, &held))
}

/// A new array of `count` elements, every one of them what is in `fill`.
pub extern "C" fn array_filled(element: u32, count: u64, fill: u64) -> u64 {
    sweep_if_asked();
    let element = Ty::from_tag(element as u8).expect("an element tag came from a type");
    u64::from(luarust_core::heap::make(element, count as usize, &cell(fill)))
}

/// One element into a cell, for the kinds compiled code cannot hold.
pub extern "C" fn array_get(handle: u64, at: u64, dst: u64) {
    let value = luarust_core::heap::read(handle as u32, at as usize)
        .expect("compiled code checks the range before asking");
    put(dst, value);
}

/// A cell into one element, likewise.
pub extern "C" fn array_put(handle: u64, at: u64, src: u64) {
    let value = cell(src);
    luarust_core::heap::store(handle as u32, at as usize, &value);
}

/// A packed value into a cell, for the times compiled code has to hand one over.
pub extern "C" fn cell_from_bits(dst: u64, bits: u64, tag: u32) {
    let ty = Ty::from_tag(tag as u8).expect("a tag came from a type");
    let value = if ty == Ty::Bool {
        Value::Bool(bits != 0)
    } else if ty.is_float() && !matches!(ty, Ty::B16 | Ty::B32 | Ty::B64) {
        Value::float(ty, luarust_num::Uint::from_u64(bits))
    } else {
        Value::Num { ty, bits }
    };
    put(dst, value);
}

/// An array handle into a cell, so the collector can find it.
///
/// It cannot go through `cell_from_bits`, because that takes a scalar's tag and an array
/// has none -- `Ty::tag` answers `u8::MAX` for one, and there is nowhere in a byte to put
/// a shape as well as a type. So the shape index travels instead, which is all
/// `Ty::Array` is made of.
pub extern "C" fn note_handle(dst: u64, bits: u64, shape: u32) {
    put(dst, Value::Num { ty: Ty::Array(shape as u8), bits });
}

/// A whole array, written the way every other path writes one.
///
/// The handle is not what anybody wants to see, and the elements are packed rather than
/// being values, so this is the one thing about an array that has to come back here.
pub extern "C" fn print_array(handle: u64, element: u32) {
    let element = Ty::from_tag(element as u8).expect("an element tag came from a type");
    let ty = luarust_core::ty::growable(element).expect("the type was already named");
    let written = luarust_core::heap::handle(ty, handle as u32).to_string();
    OUTPUT.with(|out| out.borrow_mut().extend_from_slice(written.as_bytes()));
}

/// Remember an index that was out of range, so the fault can name it.
pub extern "C" fn note_index(at: u64, length: u64) {
    REACHED.with(|held| *held.borrow_mut() = (at as i64 as i128, length as i128));
}

/// The index that was out of range, and how many there were.
pub fn reached() -> (i128, i128) {
    REACHED.with(|held| *held.borrow())
}

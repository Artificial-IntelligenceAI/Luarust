//! What compiled code calls back into.
//!
//! Machine code can add and divide on its own. It cannot print, read a clock, or work out
//! what `b256` division comes to — so for those it calls these, which are the same
//! functions the interpreter and the VM use. That is not a shortcut: it is what keeps the
//! three paths giving the same answers instead of nearly the same ones.

use luarust_check::value::{Fault, Overflow, Value, binary_op, compare, format_of, negate};
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

/// Lay out the cells and start the clock, for a program starting or being taken over.
///
/// The heap is *not* touched here. A program starting forgets the last run's arrays before
/// it gets this far -- the interpreter and the VM each do the same, and a path that did not
/// would inherit whatever the last one left, which is exactly what happens in
/// `luarust fuzz` where all three run in the one process. A program being taken over must
/// keep them, because they are its own and it is still using them.
///
/// `frames` is every call the VM has open, outermost first. All of them and not just the
/// one being taken over: they are the root set a collection inside compiled code walks, so
/// leaving the callers out would free an array only a caller could still reach. They are
/// also what `call_depth` counts, so a program that runs out of stack does so in the same
/// place it would have on the VM.
pub fn resume(
    constants: Vec<Value>,
    frames: Vec<Vec<Value>>,
    templates: Vec<Vec<Value>>,
    started: Instant,
) {
    // Tables installed by hand belong to no kept module, so the next kept call must
    // install its own rather than trust what a one-shot module left here. And a resumed
    // run owns every frame it was handed, so nothing stays borrowed.
    INSTALLED.with(|token| token.set(0));
    BORROWED.with(|callers| callers.borrow_mut().clear());
    OUTPUT.with(|out| out.borrow_mut().clear());
    STARTED.with(|at| *at.borrow_mut() = Some(started));
    CONSTANTS.with(|table| *table.borrow_mut() = constants);
    TEMPLATES.with(|table| *table.borrow_mut() = templates);
    PENDING.with(|queue| queue.borrow_mut().clear());
    ANSWER.with(|slot| *slot.borrow_mut() = None);
    FRAMES.with(|open| *open.borrow_mut() = frames);
}

thread_local! {
    /// Which kept module's constants and templates are installed, `0` for none. The
    /// tables never change for a given module, so a call entering the module that is
    /// already installed pays a comparison where it used to pay two clones.
    static INSTALLED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// Frames borrowed from the caller that suspended itself to make a kept call — the
    /// VM's own registers, read here only as collection roots and counted for depth.
    /// Raw pointers, because a thread-local cannot name the caller's lifetime. They are
    /// valid for exactly the span the caller spends blocked inside the call, which is
    /// exactly the span they are installed for: set by [`reenter`], cleared by
    /// [`retire`], and never survive either.
    static BORROWED: RefCell<Vec<*const Vec<Value>>> = const { RefCell::new(Vec::new()) };
}

/// [`resume`], for a kept module that is entered over and over.
///
/// `module` never being nought is the caller's business; a nought would alias the
/// nothing-installed state and reinstall on every call, which is slow rather than wrong.
///
/// `open` is every frame the caller has, borrowed rather than copied: nothing here
/// writes through them, compiled code cannot name a cell outside its own frame, and the
/// caller is suspended for the whole call — so a copy would be paid for by every call
/// and read by none. `fresh` is the callee's frame, which is written, so it is owned.
pub fn reenter(
    module: u64,
    constants: &[Value],
    templates: &[Vec<Value>],
    open: &[&Vec<Value>],
    fresh: Vec<Value>,
    started: Instant,
) {
    INSTALLED.with(|token| {
        if token.get() != module {
            CONSTANTS.with(|table| *table.borrow_mut() = constants.to_vec());
            TEMPLATES.with(|table| *table.borrow_mut() = templates.to_vec());
            token.set(module);
        }
    });
    BORROWED.with(|callers| {
        let mut callers = callers.borrow_mut();
        callers.clear();
        callers.extend(open.iter().map(|frame| *frame as *const Vec<Value>));
    });
    OUTPUT.with(|out| out.borrow_mut().clear());
    STARTED.with(|at| *at.borrow_mut() = Some(started));
    PENDING.with(|queue| queue.borrow_mut().clear());
    ANSWER.with(|slot| *slot.borrow_mut() = None);
    FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        frames.clear();
        frames.push(fresh);
    });
}

/// Forget the borrowed frames, before the call they belong to returns.
///
/// After this, nothing anywhere holds the pointers [`reenter`] took.
pub fn retire() {
    BORROWED.with(|callers| callers.borrow_mut().clear());
}

/// What a register holds, as the bits a machine register would hold.
///
/// Only ever called at an entry the VM handed over: the frame is what the VM was working
/// with, and this is how each of its values reaches the stack slot compiled code reads it
/// from. A value that lives in a cell answers nought, because the cell is already holding
/// it and the slot beside it is never looked at.
#[unsafe(export_name = "luarust_cell_bits")]
pub extern "C" fn cell_bits(index: u64) -> u64 {
    match cell(index) {
        Value::Num { bits, .. } => bits,
        Value::Bool(answer) => u64::from(answer),
        _ => 0,
    }
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
#[unsafe(export_name = "luarust_cell_stage")]
pub extern "C" fn cell_stage(index: u64) {
    let value = cell(index);
    PENDING.with(|queue| queue.borrow_mut().push(value));
}

/// Enter a function: a fresh frame, with the staged arguments still waiting.
///
/// They wait rather than being placed, because where they belong is a matter for the
/// callee: a celled parameter lives in the cell of whichever register it was given, and
/// the caller has no way to know which that is.
#[unsafe(export_name = "luarust_cells_enter")]
pub extern "C" fn cells_enter(routine: u64) {
    let frame = TEMPLATES.with(|table| table.borrow()[routine as usize].clone());
    FRAMES.with(|frames| frames.borrow_mut().push(frame));
}

/// Take the next staged argument into a cell of the frame just entered.
#[unsafe(export_name = "luarust_cell_unstage")]
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
#[unsafe(export_name = "luarust_cells_leave_with")]
pub extern "C" fn cells_leave_with(index: u64) {
    let value = cell(index);
    ANSWER.with(|slot| *slot.borrow_mut() = Some(value));
    FRAMES.with(|frames| {
        frames.borrow_mut().pop();
    });
}

/// Leave a function that had nothing to give back.
#[unsafe(export_name = "luarust_cells_leave")]
pub extern "C" fn cells_leave() {
    FRAMES.with(|frames| {
        frames.borrow_mut().pop();
    });
}

/// A machine value, as the `Value` the VM holds. The one place the two shapes meet.
pub fn held(ty: Ty, bits: u64) -> Value {
    match ty {
        Ty::Bool => Value::Bool(bits != 0),
        ty => Value::Num { ty, bits },
    }
}

/// The answer a routine left, for a caller on the Rust side rather than a compiled one.
///
/// Only ever used when compiled code was entered inside a routine and has just run it to
/// its return: the VM is the caller, and this is how the value reaches it.
pub fn answer() -> Option<Value> {
    ANSWER.with(|slot| slot.borrow_mut().take())
}

/// Take the answer a call left, into a cell of the frame that asked for it.
#[unsafe(export_name = "luarust_cell_take_answer")]
pub extern "C" fn cell_take_answer(dst: u64) {
    let value = ANSWER.with(|slot| slot.borrow_mut().take()).expect("a call left an answer");
    put(dst, value);
}

/// How deep the frames go, so compiled code can stop where the other two paths stop.
/// The borrowed frames are calls the caller has open, so they count.
#[unsafe(export_name = "luarust_call_depth")]
pub extern "C" fn call_depth() -> u64 {
    let borrowed = BORROWED.with(|callers| callers.borrow().len() as u64);
    borrowed + FRAMES.with(|frames| frames.borrow().len() as u64)
}

/// Copy one cell into another.
#[unsafe(export_name = "luarust_cell_move")]
pub extern "C" fn cell_move(dst: u64, src: u64) {
    let value = cell(src);
    put(dst, value);
}

/// Arithmetic on two cells, into a third. Reading happens before writing, so the
/// destination may be one of the sources.
#[unsafe(export_name = "luarust_cell_binary")]
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
#[unsafe(export_name = "luarust_cell_neg")]
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
#[unsafe(export_name = "luarust_cell_compare")]
pub extern "C" fn cell_compare(a: u64, b: u64) -> i32 {
    match compare(&cell(a), &cell(b)) {
        Comparison::Less => 0,
        Comparison::Equal => 1,
        Comparison::Greater => 2,
        Comparison::Unordered => 3,
    }
}

/// Print a cell, the way every other execution path prints one.
#[unsafe(export_name = "luarust_print_cell")]
pub extern "C" fn print_cell(index: u64) {
    let text = cell(index).to_string();
    OUTPUT.with(|out| out.borrow_mut().extend_from_slice(text.as_bytes()));
}

/// Read the clock into a cell, in whichever float format it was asked for.
#[unsafe(export_name = "luarust_cell_time_now")]
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
pub fn value_of(tag: u32, bits: u64) -> Value {
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
#[unsafe(export_name = "luarust_print_text")]
pub unsafe extern "C" fn print_text(ptr: *const u8, len: u64) {
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    OUTPUT.with(|out| out.borrow_mut().extend_from_slice(bytes));
}

/// Print a number, the way every other execution path prints one.
#[unsafe(export_name = "luarust_print_value")]
pub extern "C" fn print_value(bits: u64, tag: u32) {
    let text = value_of(tag, bits).to_string();
    OUTPUT.with(|out| out.borrow_mut().extend_from_slice(text.as_bytes()));
}

/// Seconds since the program began, in whichever float format was asked for.
///
/// Takes the type rather than always answering in `b64`, because a `b32` variable holding
/// a `b64`'s bits is a value that is not what it says it is.
#[unsafe(export_name = "luarust_time_now")]
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
#[unsafe(export_name = "luarust_compare")]
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
#[unsafe(export_name = "luarust_fallback")]
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
#[unsafe(export_name = "luarust_array_base")]
pub extern "C" fn array_base(handle: u64) -> *mut u8 {
    luarust_core::heap::base_of(handle as u32).0
}

/// How many elements an array holds.
#[unsafe(export_name = "luarust_array_len")]
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
    BORROWED.with(|callers| {
        FRAMES.with(|frames| {
            let callers = callers.borrow();
            let frames = frames.borrow();
            // SAFETY: the pointers were taken by `reenter` from references whose owner
            // is suspended for the whole of the call, and they are cleared before it
            // returns. Nothing writes through them; the collector only reads.
            let roots = callers.iter().flat_map(|frame| unsafe { (**frame).iter() });
            luarust_core::heap::collect(roots.chain(frames.iter().flatten()));
        });
    });
}

#[unsafe(export_name = "luarust_array_new")]
pub extern "C" fn array_new(element: u32, first: u64, count: u64) -> u64 {
    sweep_if_asked();
    let element = Ty::from_tag(element as u8).expect("an element tag came from a type");
    let held: Vec<Value> = (0..count).map(|n| cell(first + n)).collect();
    u64::from(luarust_core::heap::of(element, &held))
}

/// A new array of `count` elements, every one of them what is in `fill`.
#[unsafe(export_name = "luarust_array_filled")]
pub extern "C" fn array_filled(element: u32, count: u64, fill: u64) -> u64 {
    sweep_if_asked();
    let element = Ty::from_tag(element as u8).expect("an element tag came from a type");
    u64::from(luarust_core::heap::make(element, count as usize, &cell(fill)))
}

/// One element into a cell, for the kinds compiled code cannot hold.
#[unsafe(export_name = "luarust_array_get")]
pub extern "C" fn array_get(handle: u64, at: u64, dst: u64) {
    let value = luarust_core::heap::read(handle as u32, at as usize)
        .expect("compiled code checks the range before asking");
    put(dst, value);
}

/// A cell into one element, likewise.
#[unsafe(export_name = "luarust_array_put")]
pub extern "C" fn array_put(handle: u64, at: u64, src: u64) {
    let value = cell(src);
    luarust_core::heap::store(handle as u32, at as usize, &value);
}

/// A packed value into a cell, for the times compiled code has to hand one over.
#[unsafe(export_name = "luarust_cell_from_bits")]
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
#[unsafe(export_name = "luarust_note_handle")]
pub extern "C" fn note_handle(dst: u64, bits: u64, shape: u32) {
    put(dst, Value::Num { ty: Ty::Array(shape as u8), bits });
}

/// A whole array, written the way every other path writes one.
///
/// The handle is not what anybody wants to see, and the elements are packed rather than
/// being values, so this is the one thing about an array that has to come back here.
#[unsafe(export_name = "luarust_print_array")]
pub extern "C" fn print_array(handle: u64, element: u32) {
    let element = Ty::from_tag(element as u8).expect("an element tag came from a type");
    let ty = luarust_core::ty::growable(element).expect("the type was already named");
    let written = luarust_core::heap::handle(ty, handle as u32).to_string();
    OUTPUT.with(|out| out.borrow_mut().extend_from_slice(written.as_bytes()));
}

/// Remember an index that was out of range, so the fault can name it.
#[unsafe(export_name = "luarust_note_index")]
pub extern "C" fn note_index(at: u64, length: u64) {
    REACHED.with(|held| *held.borrow_mut() = (at as i64 as i128, length as i128));
}

/// The index that was out of range, and how many there were.
pub fn reached() -> (i128, i128) {
    REACHED.with(|held| *held.borrow())
}


/// What a fault code means, without reference to where it happened.
///
/// The code and the place are separate on purpose. Compiled code carries a code out in its
/// return value; the *place* lives in a span table the emitter built, which the in-memory
/// JIT still has and a program compiled to a file does not. So a native binary can say
/// what went wrong even though it cannot yet say which line.
pub fn fault_of(outcome: i64) -> Fault {
    let code = outcome & 0xff;
    match code {
        DIVIDE_BY_ZERO => Fault {
            code: "R0002",
            message: "this divides a whole number by zero.".into(),
            rule: "an integer has no way to express what dividing by zero would give",
            fix: "check the divisor before dividing, or use a float type, where it is an infinity."
                .into(),
        },
        REMAINDER_BY_ZERO => Fault {
            code: "R0003",
            message: "this takes a remainder against zero.".into(),
            rule: "a remainder against zero is not a number",
            fix: "check the divisor before taking a remainder.".into(),
        },
        OUT_OF_RANGE => {
            let (at, length) = reached();
            Fault {
                code: "R0015",
                message: format!("there is no element {at} here."),
                rule: "an array is counted from one, up to how many it holds",
                fix: if length == 0 {
                    "this one holds nothing at all.".to_string()
                } else {
                    format!("this one holds {length}, so the last is {length} and the first is 1.")
                },
            }
        }
        FRACTIONAL_POWER => Fault {
            code: "R0012",
            message: "this raises an exact number to a power that is not whole.".into(),
            rule: "a ratio raised to a whole power is a ratio, and raised to anything else usually is not",
            fix: "use a whole exponent, or a float type, where the answer can be approximated."
                .into(),
        },
        POWER_TOO_LARGE => Fault {
            code: "R0013",
            message: format!(
                "this raises an exact number to a power above {}.",
                luarust_num::Exact::POWER_LIMIT
            ),
            rule: "an exact answer has to be written down, and that one would not fit anywhere",
            fix: "use a smaller exponent, or a float type, where the answer is rounded to a width."
                .into(),
        },
        TOO_DEEP => Fault {
            code: "R0011",
            message: format!(
                "this has called itself {} deep.",
                luarust_check::value::DEPTH_LIMIT
            ),
            rule: "a call may only go so deep before the program is stopped",
            fix: "give the recursion a case that stops, or write it as a loop.".into(),
        },
        DOES_NOT_FIT => Fault {
            code: "R0005",
            message: "this does not fit the width it is stored at.".into(),
            rule: "with overflow set to trap, a whole number must fit the width it is stored at",
            fix: "use a wider type, or let overflow wrap.".into(),
        },
        _ => Fault {
            code: "R0011",
            message: "the compiled program stopped.".into(),
            rule: "a program stops when an operation has no answer",
            fix: "run it with `luarust interp` to find out what happened.".into(),
        },
    }
}

/// One line saying what went wrong, for a program with no diagnostics machinery linked in.
pub fn fault_text(outcome: i64) -> String {
    let fault = fault_of(outcome);
    format!("{}\n\n{}\nRule(s) broken: {}\nSuggested fix(s): {}",
            fault.code, fault.message, fault.rule, fault.fix)
}

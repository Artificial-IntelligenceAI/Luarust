//! What compiled code calls back into.
//!
//! Machine code can add and divide on its own. It cannot print, read a clock, or work out
//! what `b256` division comes to — so for those it calls these, which are the same
//! functions the interpreter and the VM use. That is not a shortcut: it is what keeps the
//! three paths giving the same answers instead of nearly the same ones.

use luarust_check::value::{Overflow, Value, binary_op, format_of};
use luarust_num::binary::{self, Round};
use luarust_parse::ast::{BinOp, Ty};
use std::cell::RefCell;
use std::time::Instant;

thread_local! {
    /// What the running program has printed. Collected rather than streamed, because a
    /// callback cannot easily be handed the caller's writer.
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    /// When the program started, for the clock.
    static STARTED: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

/// Begin a run: forget the last one's output and restart the clock.
pub fn begin() {
    OUTPUT.with(|out| out.borrow_mut().clear());
    STARTED.with(|at| *at.borrow_mut() = Some(Instant::now()));
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
        _ => u32::MAX,
    }
}

fn untag(tag: u32) -> Ty {
    match tag {
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
    Value::Num { ty: untag(tag), bits }
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

/// Seconds since the program began, as `b64` bits.
pub extern "C" fn time_now() -> u64 {
    let seconds = STARTED.with(|at| {
        at.borrow().map(|started| started.elapsed().as_secs_f64()).unwrap_or(0.0)
    });
    let fmt = format_of(Ty::B64).expect("b64 has a format");
    binary::from_decimal::<8>(fmt, Round::TiesToEven, &format!("{seconds:.9}"))
        .expect("nine decimal places is a number")
        .low64()
}

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
        Err(fault) => match fault.code {
            "R0002" => DIVIDE_BY_ZERO,
            "R0003" => REMAINDER_BY_ZERO,
            "R0005" => DOES_NOT_FIT,
            _ => OTHER,
        },
    }
}

//! What a compiled-ahead-of-time program links against.
//!
//! The in-memory JIT hands the runtime its tables as live Rust values and reads the answer
//! back as a return code. A program written to a file has nobody to do either for it, so
//! the emitter generates a `main` that calls into here: [`luarust_start`] to lay the tables
//! out, then the compiled `luarust_main`, then [`luarust_finish`] to say what happened.
//!
//! Nothing here compiles anything. That is the whole point of the arrangement -- LLVM is
//! thirty-two megabytes and belongs to the machine that *built* the program, not the one
//! that runs it.

use std::io::Write;

/// Lay out the tables a compiled program needs, from the bytes it carries.
///
/// # Safety
/// `tables` must point at `len` readable bytes, which the emitter put in the binary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luarust_start(tables: *const u8, len: u64, collect: u32, floats: u32, division: u32) {
    let bytes = unsafe { std::slice::from_raw_parts(tables, len as usize) };
    let (constants, frame, templates) = match luarust_vm::serialize::read_tables(bytes) {
        Ok(tables) => tables,
        // The bytes are inside this executable. If they are wrong, the executable is
        // damaged, and there is nothing to run.
        Err(why) => {
            let _ = writeln!(std::io::stderr(), "this program's own tables are damaged: {why}");
            std::process::exit(70);
        }
    };
    if let Some(how) = luarust_core::heap::Collect::from_tag(collect) {
        luarust_core::heap::set_threshold(how.threshold());
    }
    if let Some(how) = luarust_core::value::Floats::from_tag(floats) {
        luarust_core::value::set_floats(how);
    }
    if let Some(how) = luarust_core::value::Division::from_tag(division) {
        luarust_core::value::set_division(how);
    }
    // A run starts with nothing, the way it does on every other path.
    luarust_core::heap::clear();
    luarust_runtime::resume(constants, vec![frame], templates, std::time::Instant::now());
}

/// Write out whatever the program printed, report a fault if there was one, and say what
/// the process should exit with.
#[unsafe(no_mangle)]
pub extern "C" fn luarust_finish(outcome: i64) -> i32 {
    let out = std::io::stdout();
    let mut out = out.lock();
    let _ = out.write_all(&luarust_runtime::taken());
    let _ = out.flush();
    if outcome == 0 {
        return 0;
    }
    // Without the emitter's span table there is no line to point at, so it says what went
    // wrong and not where. Carrying the spans is the next thing this wants.
    let _ = writeln!(std::io::stderr(), "\n{}", luarust_runtime::fault_text(outcome));
    1
}

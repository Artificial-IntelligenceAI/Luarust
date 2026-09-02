//! The program that runs a program.
//!
//! This is what a machine that only has to *run* Luarust needs to have on it. It reads a
//! `.lrc` chunk and runs it. It cannot compile, because a chunk is already compiled; it
//! cannot report on source it was not given, because a chunk built without its source
//! does not carry any. What it will not do is carry a lexer, a parser, a checker or a
//! program generator to the device, on the chance that one day it might.
//!
//! A JIT it will carry, if asked for one: `--features jit`. That is not the same thing as
//! carrying it on the chance — a build that does not ask still has no compiler in it at
//! all, and the default does not ask. What the feature buys is that shipping a program
//! which wants `[run] mode = "hot"` means shipping a *runtime*, rather than shipping the
//! toolchain, which would put a lexer, a parser, a checker, a disassembler and a program
//! generator on a machine that only ever had to run one program.
//!
//! `luarust` is the toolchain and belongs on the machine where the program is written.
//! This is the other half, and it is about forty times smaller.

use std::io::Write;
use std::process::ExitCode;

/// Run a chunk the way it asked to be run, as far as this build is able.
///
/// A `[run] mode` is a preference and not an instruction, so a build with no compiler in
/// it runs the VM rather than refusing — but it says so first. Refusing to run a program
/// because the fastest way of running it is unavailable would help nobody; running it
/// several times slower than it asked for and saying nothing is a different thing, and
/// was how this behaved until it was noticed that `luarust jit` on such a build explains
/// itself at length while a chunk asking for the same thing got silence.
fn engine(
    chunk: &luarust_vm::Chunk,
    out: &mut impl Write,
) -> Result<(), luarust_core::value::Stopped> {
    #[cfg(feature = "jit")]
    {
        use luarust_core::value::Engine;
        if chunk.engine == Engine::Whole {
            match luarust_jit::run(chunk, out) {
                Ok(outcome) => return outcome,
                Err(declined) => eprintln!(
                    "the JIT declined this program: {}. Running it on the VM.",
                    declined.because
                ),
            }
        }
        if chunk.engine == Engine::Hot {
            let mut tier = luarust_jit::Compiling::new();
            return luarust_vm::run_with(chunk, out, Some(&mut tier));
        }
    }
    luarust_vm::run(chunk, out)
}

/// Say what the chunk asked for that this runtime has not got, and whether that is fatal.
#[cfg(feature = "jit")]
fn engine_check(_chunk: &luarust_vm::Chunk) -> Result<(), ExitCode> {
    Ok(())
}

#[cfg(not(feature = "jit"))]
fn engine_check(chunk: &luarust_vm::Chunk) -> Result<(), ExitCode> {
    match luarust_vm::without_a_compiler(
        chunk,
        "this runtime",
        "cargo build --release -p luarust-run --features jit",
    ) {
        luarust_vm::Without::Fine => Ok(()),
        luarust_vm::Without::FallingBack(say) => {
            eprintln!("{say}");
            Ok(())
        }
        luarust_vm::Without::Refused(say) => {
            eprintln!("{say}");
            Err(ExitCode::from(2))
        }
    }
}

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("luarust-run <file.lrc>");
        return ExitCode::from(2);
    };

    // What this runtime can honour, one engine per line, for whoever is about to bundle
    // it. A build asking is the only caller, and it needs a machine-readable answer
    // rather than a size on disk -- the two runtimes differ by a cargo feature, not by
    // anything visible in the file.
    if path == "--engines" {
        println!("vm");
        #[cfg(feature = "jit")]
        {
            println!("whole");
            println!("hot");
        }
        return ExitCode::SUCCESS;
    }

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(why) => {
            eprintln!("{path} could not be read: {why}");
            return ExitCode::from(2);
        }
    };

    // Source is not accepted, and not because it would be hard: a machine that runs
    // chunks should not be the machine that decides whether a program is well formed.
    if !bytes.starts_with(luarust_vm::serialize::MAGIC) {
        eprintln!("{path} is not a Luarust chunk. Compile it first: luarust build {path}");
        return ExitCode::from(2);
    }

    let loaded = match luarust_vm::serialize::read(&bytes) {
        Ok(loaded) => loaded,
        Err(broken) => {
            eprintln!("{path}: {broken}");
            return ExitCode::from(2);
        }
    };

    if let Err(code) = engine_check(&loaded.chunk) {
        return code;
    }

    let mut out = std::io::stdout().lock();
    let outcome = engine(&loaded.chunk, &mut out);
    let _ = out.flush();

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(stopped) => {
            // A chunk built without its source still knows the line: what it cannot do is
            // show it, and the renderer says so rather than pretending the line is blank.
            let source = loaded.source.file(loaded.path.clone());
            eprint!("{}", luarust_diag::report(&source, &[stopped.diagnostic()]));
            ExitCode::FAILURE
        }
    }
}

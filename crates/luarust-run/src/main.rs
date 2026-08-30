//! The program that runs a program.
//!
//! This is what a machine that only has to *run* Luarust needs to have on it. It reads a
//! `.lrc` chunk and runs it. It cannot compile, because a chunk is already compiled; it
//! cannot report on source it was not given, because a chunk built without its source
//! does not carry any. What it will not do is carry a lexer, a parser, a checker, a
//! program generator or a JIT to the device, on the chance that one day it might.
//!
//! `luarust` is the toolchain and belongs on the machine where the program is written.
//! This is the other half, and it is about forty times smaller.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("luarust-run <file.lrc>");
        return ExitCode::from(2);
    };

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

    let mut out = std::io::stdout().lock();
    let outcome = luarust_vm::run(&loaded.chunk, &mut out);
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

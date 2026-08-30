//! The `luarust` command.

use luarust_diag::{Diagnostic, SourceFile};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
luarust — a language that would rather not guess

    luarust run <file.lr>       compile to bytecode and run it
    luarust interp <file.lr>    run it on the reference interpreter instead
    luarust verify <file.lr>    run it both ways and report whether they agree
    luarust dis <file.lr>       show what the compiler decided
    luarust build <file.lr>     compile it to a .lrc chunk and stop
    luarust check <file.lr>     check it and stop
    luarust fuzz [count]        write programs and check the paths agree
    luarust --help              this
";

/// Only there when the JIT was built in, which needs LLVM.
#[cfg(feature = "jit")]
const JIT_USAGE: &str = "\
    luarust jit <file.lr>       compile it with LLVM, in memory, and run it
    luarust ir <file.lr>        show the LLVM IR
";

/// What to do once a file has checked out.
enum Then {
    Nothing,
    Run,
    Interpret,
    Verify,
    Build,
    Disassemble,
    #[cfg(feature = "jit")]
    Jit,
    #[cfg(feature = "jit")]
    Ir,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let path = args.next().map(PathBuf::from);

    let then = match command.as_deref() {
        Some("run") => Then::Run,
        Some("interp") => Then::Interpret,
        Some("verify") => Then::Verify,
        Some("build") => Then::Build,
        Some("dis") => Then::Disassemble,
        #[cfg(feature = "jit")]
        Some("jit") => Then::Jit,
        #[cfg(feature = "jit")]
        Some("ir") => Then::Ir,
        Some("check") => Then::Nothing,
        Some("fuzz") => {
            let count = path
                .as_ref()
                .and_then(|n| n.to_string_lossy().parse::<u64>().ok())
                .unwrap_or(1000);
            return fuzz(count);
        }
        Some("--help" | "-h" | "help") => {
            print!("{USAGE}");
            #[cfg(feature = "jit")]
            print!("{JIT_USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprint!("{USAGE}");
            #[cfg(feature = "jit")]
            eprint!("{JIT_USAGE}");
            return ExitCode::from(2);
        }
    };

    let Some(path) = path else {
        eprintln!("that needs a file to work on.\n\n{USAGE}");
        return ExitCode::from(2);
    };
    act(path, then)
}

fn act(path: PathBuf, then: Then) -> ExitCode {
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(why) => {
            eprintln!("{} could not be read: {why}", path.display());
            return ExitCode::from(2);
        }
    };

    // A chunk is recognised by what it begins with rather than by what it is called, so a
    // renamed file still works and a source file that happens to end in .lrc still does.
    if bytes.starts_with(luarust_vm::serialize::MAGIC) {
        return run_chunk(&bytes, &path, then);
    }

    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            eprintln!("{} is neither Luarust source nor a Luarust chunk.", path.display());
            return ExitCode::from(2);
        }
    };
    let source = SourceFile::new(path.clone(), text);

    // Each stage runs only if the one before it had nothing to say, because a parser fed
    // broken tokens invents problems that are not really there.
    let mut errors: Vec<Diagnostic> = Vec::new();
    let lexed = luarust_lex::lex(source.text());
    errors.extend(lexed.errors);

    let mut checked = None;
    if errors.is_empty() {
        let parsed = luarust_parse::parse(source.text(), &lexed.tokens);
        errors.extend(parsed.errors);
        if errors.is_empty() {
            let (program, problems) = luarust_check::check(&parsed.program);
            errors.extend(problems);
            checked = Some(program);
        }
    }

    if !errors.is_empty() {
        eprint!("{}", luarust_diag::report(&source, &errors));
        return ExitCode::FAILURE;
    }
    let Some(program) = checked else { return ExitCode::FAILURE };

    match then {
        Then::Nothing => ExitCode::SUCCESS,

        Then::Disassemble => {
            print!("{}", luarust_vm::compile(&program).disassemble());
            ExitCode::SUCCESS
        }

        Then::Build => {
            let chunk = luarust_vm::compile(&program);
            let bytes = luarust_vm::serialize::write(
                &chunk,
                &path.display().to_string(),
                source.text(),
            );
            let out = path.with_extension("lrc");
            match std::fs::write(&out, &bytes) {
                Ok(()) => {
                    println!("{} — {} bytes", out.display(), bytes.len());
                    ExitCode::SUCCESS
                }
                Err(why) => {
                    eprintln!("{} could not be written: {why}", out.display());
                    ExitCode::from(2)
                }
            }
        }

        Then::Run => {
            let chunk = luarust_vm::compile(&program);
            let mut out = std::io::stdout().lock();
            finish(luarust_vm::run(&chunk, &mut out), &mut out, &source)
        }

        Then::Interpret => {
            let mut out = std::io::stdout().lock();
            finish(luarust_interp::run(&program, &mut out), &mut out, &source)
        }

        Then::Verify => verify(&program, &source),

        // The JIT declines more than it takes, so a program it will not compile falls
        // back to the VM rather than failing.
        #[cfg(feature = "jit")]
        Then::Jit => {
            let mut out = std::io::stdout().lock();
            match luarust_jit::run(&program, &mut out) {
                Ok(outcome) => finish(outcome, &mut out, &source),
                Err(declined) => {
                    eprintln!("the JIT declined this program: {}. Running it on the VM.", declined.because);
                    let chunk = luarust_vm::compile(&program);
                    finish(luarust_vm::run(&chunk, &mut out), &mut out, &source)
                }
            }
        }

        #[cfg(feature = "jit")]
        Then::Ir => match luarust_jit::emit_ir(&program) {
            Ok(ir) => {
                print!("{ir}");
                ExitCode::SUCCESS
            }
            Err(declined) => {
                eprintln!("the JIT declined this program: {}", declined.because);
                ExitCode::FAILURE
            }
        },
    }
}

/// Run, or look at, a chunk that came off disk.
///
/// Nothing is lexed, parsed or checked: the file *is* the program. The source it was
/// compiled from travels inside it, which is what lets a program that stops half way
/// through still point at the line that did it, on a machine that has never seen the
/// source.
fn run_chunk(bytes: &[u8], path: &Path, then: Then) -> ExitCode {
    let loaded = match luarust_vm::serialize::read(bytes) {
        Ok(loaded) => loaded,
        Err(broken) => {
            eprintln!("{}: {broken}", path.display());
            return ExitCode::from(2);
        }
    };

    match then {
        Then::Disassemble => {
            print!("{}", loaded.chunk.disassemble());
            ExitCode::SUCCESS
        }
        Then::Nothing => {
            println!("{} — a Luarust chunk, and it checks out.", path.display());
            ExitCode::SUCCESS
        }
        Then::Run => {
            let source = SourceFile::new(loaded.path.clone(), loaded.source.clone());
            let mut out = std::io::stdout().lock();
            finish(luarust_vm::run(&loaded.chunk, &mut out), &mut out, &source)
        }
        _ => {
            eprintln!(
                "{} is a chunk, so it can only be `run`, `dis`, or `check`. \
                 The other commands need the source.",
                path.display()
            );
            ExitCode::from(2)
        }
    }
}

fn finish(
    outcome: Result<(), luarust_check::value::Stopped>,
    out: &mut impl Write,
    source: &SourceFile,
) -> ExitCode {
    let _ = out.flush();
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(stopped) => {
            eprint!("{}", luarust_diag::report(source, &[stopped.diagnostic()]));
            ExitCode::FAILURE
        }
    }
}

/// Run a program both ways and say whether the two agree.
///
/// This is the whole reason the interpreter is kept after the VM exists. One
/// implementation only ever agrees with itself.
fn verify(program: &luarust_check::ir::Checked, source: &SourceFile) -> ExitCode {
    let mut walked = Vec::new();
    let walk = luarust_interp::run(program, &mut walked);

    let chunk = luarust_vm::compile(program);
    let mut ran = Vec::new();
    let vm = luarust_vm::run(&chunk, &mut ran);

    let same_output = walked == ran;
    let same_ending = match (&walk, &vm) {
        (Ok(()), Ok(())) => true,
        (Err(a), Err(b)) => a.fault.code == b.fault.code,
        _ => false,
    };

    if same_output && same_ending {
        println!("the interpreter and the VM agree.");
        if let Err(stopped) = walk {
            println!("both stopped: {} — {}", stopped.fault.code, stopped.fault.message);
        }
        return ExitCode::SUCCESS;
    }

    println!("the interpreter and the VM DISAGREE.");
    if !same_output {
        println!("\ninterpreter printed:\n{}", String::from_utf8_lossy(&walked));
        println!("the VM printed:\n{}", String::from_utf8_lossy(&ran));
    }
    if !same_ending {
        println!("\ninterpreter: {}", ending(&walk));
        println!("the VM:      {}", ending(&vm));
    }
    println!("\n{}", chunk.disassemble());
    let _ = source;
    ExitCode::FAILURE
}

/// Write programs and insist the two paths agree about every one of them.
///
/// Type-directed, so every generated program compiles -- one that did not would be
/// rejected identically by both paths and would prove nothing.
fn fuzz(count: u64) -> ExitCode {
    let mut ran = 0u64;
    let mut stopped = 0u64;

    for seed in 1..=count {
        let written = luarust_gen::program(seed);
        let source = SourceFile::new(format!("seed-{seed}.lr"), written.source.clone());

        let lexed = luarust_lex::lex(source.text());
        let parsed = luarust_parse::parse(source.text(), &lexed.tokens);
        let (program, errors) = luarust_check::check(&parsed.program);
        let refused: Vec<_> =
            lexed.errors.into_iter().chain(parsed.errors).chain(errors).collect();
        if !refused.is_empty() {
            println!("seed {seed}: a generated program did not compile.\n");
            print!("{}", written.source);
            print!("{}", luarust_diag::report(&source, &refused));
            return ExitCode::FAILURE;
        }

        let mut walked = Vec::new();
        let walk = luarust_interp::run(&program, &mut walked);
        let chunk = luarust_vm::compile(&program);
        let mut vm_out = Vec::new();
        let vm = luarust_vm::run(&chunk, &mut vm_out);

        let same_ending = match (&walk, &vm) {
            (Ok(()), Ok(())) => true,
            (Err(a), Err(b)) => a.fault.code == b.fault.code,
            _ => false,
        };
        if walked != vm_out || !same_ending {
            println!("seed {seed}: the two paths DISAGREE.\n");
            print!("{}", written.source);
            println!("\ninterpreter printed:\n{}", String::from_utf8_lossy(&walked));
            println!("the VM printed:\n{}", String::from_utf8_lossy(&vm_out));
            println!("\ninterpreter: {}", ending(&walk));
            println!("the VM:      {}", ending(&vm));
            println!("\n{}", chunk.disassemble());
            return ExitCode::FAILURE;
        }

        ran += 1;
        if walk.is_err() {
            stopped += 1;
        }
    }

    println!(
        "{ran} programs, all compiled, all agreed. {stopped} of them stopped part way, \
         and stopped the same way both times."
    );
    ExitCode::SUCCESS
}

fn ending(outcome: &Result<(), luarust_check::value::Stopped>) -> String {
    match outcome {
        Ok(()) => "ran to the end".to_string(),
        Err(stopped) => format!("stopped with {}", stopped.fault.code),
    }
}

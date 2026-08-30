//! The `luarust` command.

use luarust_diag::{Diagnostic, SourceFile};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
luarust — a language that would rather not guess

    luarust run <file.lr>       compile to bytecode and run it
    luarust interp <file.lr>    run it on the reference interpreter instead
    luarust verify <file.lr>    run it both ways and report whether they agree
    luarust dis <file.lr>       show what the compiler decided
    luarust check <file.lr>     check it and stop
    luarust --help              this
";

/// What to do once a file has checked out.
enum Then {
    Nothing,
    Run,
    Interpret,
    Verify,
    Disassemble,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let path = args.next().map(PathBuf::from);

    let then = match command.as_deref() {
        Some("run") => Then::Run,
        Some("interp") => Then::Interpret,
        Some("verify") => Then::Verify,
        Some("dis") => Then::Disassemble,
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
            return ExitCode::SUCCESS;
        }
        _ => {
            eprint!("{USAGE}");
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
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(why) => {
            eprintln!("{} could not be read: {why}", path.display());
            return ExitCode::from(2);
        }
    };
    let source = SourceFile::new(path, text);

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

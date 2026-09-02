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
    luarust native <file.lr>    compile it to a program that runs on its own
    luarust native <file.lr> --for <target>    ... for a different machine
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
    #[cfg(feature = "jit")]
    Native,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let path = args.next().map(PathBuf::from);
    // `luarust native file.lr --for x86_64-unknown-linux-gnu`. Only `native` reads it:
    // a chunk already runs anywhere, so only a program being turned into machine code has
    // a machine to be turned into it *for*.
    let mut wanted: Option<String> = None;
    while let Some(arg) = args.next() {
        if arg == "--for" {
            wanted = args.next();
        }
    }
    let target = wanted;

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
        #[cfg(feature = "jit")]
        Some("native") => Then::Native,
        Some("check") => Then::Nothing,

        // A build without the JIT still knows the word, so that asking for something it
        // was built without is answered rather than met with the usage text. Printing the
        // usage says "you typed something wrong", and the typing was fine.
        #[cfg(not(feature = "jit"))]
        Some(asked @ ("jit" | "ir" | "native")) => {
            eprintln!(
                "this build has no JIT, so it cannot `{asked}`. It was built without the \
                 `jit` feature:\n\n    \
                 cargo build --release -p luarust-cli --features jit\n\n\
                 which needs LLVM 21. `luarust run` works either way."
            );
            return ExitCode::from(2);
        }
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
    act(path, then, target.as_deref())
}

fn act(path: PathBuf, then: Then, target: Option<&str>) -> ExitCode {
    // Only the native path asks for a target, and a build without the JIT has no
    // native path to hand it to.
    #[cfg(not(feature = "jit"))]
    let _ = target;
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

    // A project file is checked as a project file and not as a program.
    //
    // Without this, naming one lexes it as Luarust: it has words, `=`, quotes and square
    // brackets, so it gets a surprising distance before something gives it away -- `#`,
    // which is a comment in TOML and nothing at all here. The error was then E0003, from
    // the lexer, about a file the lexer had no business reading.
    if path.file_name().is_some_and(|name| name == luarust_conf::FILENAME) {
        let (_, errors) = luarust_conf::read(source.text());
        if errors.is_empty() {
            if matches!(then, Then::Nothing) {
                println!("{} says nothing wrong.", path.display());
            }
            return ExitCode::SUCCESS;
        }
        eprint!("{}", luarust_diag::report(&source, &errors));
        return ExitCode::FAILURE;
    }

    // What the project decided applies to every file in it. A `defaults.` line inside the
    // file still overrules it, which the checker handles by starting from these.
    let project = match project_settings(&path) {
        Ok(project) => project,
        Err(code) => return code,
    };

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
            let start = luarust_check::Start {
                overflow: project.overflow,
                visibility_required: project.visibility_required,
                collect: match project.gc {
                    luarust_conf::Collect::Off => luarust_core::heap::Collect::Off,
                    luarust_conf::Collect::Silent => luarust_core::heap::Collect::Silent,
                    luarust_conf::Collect::Aggressive => luarust_core::heap::Collect::Aggressive,
                },
                floats: match project.floats {
                    luarust_conf::Floats::Exact => luarust_core::value::Floats::Exact,
                    luarust_conf::Floats::Shortest => luarust_core::value::Floats::Shortest,
                },
                engine: match project.engine {
                    luarust_conf::Engine::Vm => luarust_core::value::Engine::Vm,
                    luarust_conf::Engine::Whole => luarust_core::value::Engine::Whole,
                    luarust_conf::Engine::Hot => luarust_core::value::Engine::Hot,
                },
                insistence: match project.insistence {
                    luarust_conf::Insistence::Optional => luarust_core::value::Insistence::Optional,
                    luarust_conf::Insistence::Required => luarust_core::value::Insistence::Required,
                    luarust_conf::Insistence::Bundled => luarust_core::value::Insistence::Bundled,
                },
                division: match project.division {
                    luarust_conf::Division::Floored => luarust_core::value::Division::Floored,
                    luarust_conf::Division::Truncated => luarust_core::value::Division::Truncated,
                    luarust_conf::Division::Euclidean => luarust_core::value::Division::Euclidean,
                },
            };
            let (program, problems) = luarust_check::check_with(&parsed.program, start);
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

        #[cfg(feature = "jit")]
        Then::Native => {
            let chunk = luarust_vm::compile(&program);
            native(
                &chunk,
                &path,
                project.target_cpu == luarust_conf::TargetCpu::ThisMachine,
                target,
            )
        }

        Then::Build => {
            let chunk = luarust_vm::compile(&program);
            let bytes = luarust_vm::serialize::write_with(
                &chunk,
                &path.display().to_string(),
                source.text(),
                project.embed_source,
                project.dpd,
            );
            let out = path.with_extension("lrc");
            match std::fs::write(&out, &bytes) {
                Ok(()) => {
                    println!("{} — {} bytes", out.display(), bytes.len());
                    if project.insistence == luarust_conf::Insistence::Bundled
                        && let Err(code) = bundle_runtime(&chunk, &out)
                    {
                        return code;
                    }
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
            if let Err(code) = engine_check(&chunk) {
                return code;
            }
            let mut out = std::io::stdout().lock();
            finish(run_as_asked(&chunk, &mut out), &mut out, &source)
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
            let chunk = luarust_vm::compile(&program);
            let mut out = std::io::stdout().lock();
            match luarust_jit::run(&chunk, &mut out) {
                Ok(outcome) => finish(outcome, &mut out, &source),
                Err(declined) => {
                    eprintln!("the JIT declined this program: {}. Running it on the VM.", declined.because);
                    finish(luarust_vm::run(&chunk, &mut out), &mut out, &source)
                }
            }
        }

        #[cfg(feature = "jit")]
        Then::Ir => match luarust_jit::emit_ir(&luarust_vm::compile(&program)) {
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
            if let Err(code) = engine_check(&loaded.chunk) {
                return code;
            }
            let source = loaded.source.file(loaded.path.clone());
            let mut out = std::io::stdout().lock();
            finish(run_as_asked(&loaded.chunk, &mut out), &mut out, &source)
        }
        // The JIT reads bytecode now, so a chunk off disk is exactly what it wants. The
        // same file the VM runs, compiled to machine code instead of interpreted.
        #[cfg(feature = "jit")]
        Then::Jit => {
            let source = loaded.source.file(loaded.path.clone());
            let mut out = std::io::stdout().lock();
            match luarust_jit::run(&loaded.chunk, &mut out) {
                Ok(outcome) => finish(outcome, &mut out, &source),
                Err(declined) => {
                    eprintln!(
                        "the JIT declined this chunk: {}. Running it on the VM.",
                        declined.because
                    );
                    finish(luarust_vm::run(&loaded.chunk, &mut out), &mut out, &source)
                }
            }
        }

        #[cfg(feature = "jit")]
        Then::Ir => match luarust_jit::emit_ir(&loaded.chunk) {
            Ok(ir) => {
                print!("{ir}");
                ExitCode::SUCCESS
            }
            Err(declined) => {
                eprintln!("the JIT declined this chunk: {}", declined.because);
                ExitCode::FAILURE
            }
        },

        _ => {
            eprintln!(
                "{} is a chunk, so it can only be `run`, `jit`, `dis`, `ir` or `check`. \
                 The other commands need the source.",
                path.display()
            );
            ExitCode::from(2)
        }
    }
}

/// Run a chunk the way its project asked to have it run.
///
/// `[run] mode` is a preference, not an instruction. A build with no JIT in it runs the VM
/// whatever the chunk says, and so does a program the JIT declines — there is no sense in
/// refusing to run something because the fastest way of running it is unavailable.
fn run_as_asked(
    chunk: &luarust_vm::Chunk,
    out: &mut impl std::io::Write,
) -> Result<(), luarust_check::value::Stopped> {
    #[cfg(feature = "jit")]
    if chunk.engine == luarust_core::value::Engine::Whole {
        match luarust_jit::run(chunk, out) {
            Ok(outcome) => return outcome,
            Err(declined) if !chunk.insistence.may_fall_back() => {
                eprintln!(
                    "this program requires `[run] mode = \"whole\"` and the JIT declined \
                     it: {}.\n\nA decline is LLVM failing at something rather than the \
                     program being unsuitable, so this has not been run on the VM \
                     instead. Set `[run] engine = \"optional\"` if that is what you \
                     want.",
                    declined.because
                );
                std::process::exit(2);
            }
            Err(declined) => eprintln!(
                "the JIT declined this program: {}. Running it on the VM.",
                declined.because
            ),
        }
    }
    #[cfg(feature = "jit")]
    if chunk.engine == luarust_core::value::Engine::Hot {
        let mut tier = luarust_jit::Compiling::new();
        return luarust_vm::run_with(chunk, out, Some(&mut tier));
    }
    luarust_vm::run(chunk, out)
}

/// Put a runtime that can honour this chunk's engine beside the chunk.
///
/// `engine = "bundled"` cannot conjure a runtime any more than `luarust native` can
/// conjure a target's libc: something has to have built one already. So this looks for a
/// `luarust-run` beside the toolchain that is running, asks it what engines it has, and
/// copies it only if the answer covers what the chunk wants. Asking is the only way to
/// know — the two runtimes differ by a cargo feature and not by anything in the file.
fn bundle_runtime(chunk: &luarust_vm::Chunk, chunk_path: &Path) -> Result<(), ExitCode> {
    let wanted = match chunk.engine {
        luarust_core::value::Engine::Vm => {
            println!("  nothing to bundle: `mode = \"vm\"` needs no compiler.");
            return Ok(());
        }
        luarust_core::value::Engine::Whole => "whole",
        luarust_core::value::Engine::Hot => "hot",
    };

    let name = format!("luarust-run{}", std::env::consts::EXE_SUFFIX);
    let Some(runtime) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|beside| beside.join(&name)))
        .filter(|found| found.is_file())
    else {
        eprintln!(
            "`[run] engine = \"bundled\"` needs a `{name}` beside this toolchain to copy, \
             and there is not one. Build it:\n\n    \
             cargo build --release -p luarust-run --features jit\n"
        );
        return Err(ExitCode::from(2));
    };

    let engines = match std::process::Command::new(&runtime).arg("--engines").output() {
        Ok(said) if said.status.success() => String::from_utf8_lossy(&said.stdout).into_owned(),
        _ => {
            eprintln!("{} would not say what engines it has.", runtime.display());
            return Err(ExitCode::from(2));
        }
    };
    if !engines.lines().any(|line| line.trim() == wanted) {
        eprintln!(
            "`[run] engine = \"bundled\"` wants a runtime that can do `{wanted}`, and \
             {} has only: {}. Build one that can:\n\n    \
             cargo build --release -p luarust-run --features jit\n",
            runtime.display(),
            engines.split_whitespace().collect::<Vec<_>>().join(", ")
        );
        return Err(ExitCode::from(2));
    }

    let beside = chunk_path.with_file_name(&name);
    match std::fs::copy(&runtime, &beside) {
        Ok(bytes) => {
            println!("{} — {bytes} bytes, and it can do `{wanted}`", beside.display());
            Ok(())
        }
        Err(why) => {
            eprintln!("{} could not be written: {why}", beside.display());
            Err(ExitCode::from(2))
        }
    }
}

/// Say what a chunk asked for that this build has not got, and whether that is fatal.
///
/// A build with the JIT in it can honour anything a chunk asks for, so it never speaks.
#[cfg(feature = "jit")]
fn engine_check(_chunk: &luarust_vm::Chunk) -> Result<(), ExitCode> {
    Ok(())
}

#[cfg(not(feature = "jit"))]
fn engine_check(chunk: &luarust_vm::Chunk) -> Result<(), ExitCode> {
    match luarust_vm::without_a_compiler(
        chunk,
        "this build",
        "cargo build --release -p luarust-cli --features jit",
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

/// Find and read the `Luarust.toml` for a file, if its project has one.
///
/// A project file that is wrong stops the build. It decides how every file in the project
/// is compiled, so guessing past a mistake in it would quietly compile the wrong thing.
fn project_settings(path: &Path) -> Result<luarust_conf::Project, ExitCode> {
    let Some(found) = luarust_conf::find(path) else {
        return Ok(luarust_conf::Project::default());
    };
    let text = match std::fs::read_to_string(&found) {
        Ok(text) => text,
        Err(why) => {
            eprintln!("{} could not be read: {why}", found.display());
            return Err(ExitCode::from(2));
        }
    };

    let (project, errors) = luarust_conf::read(&text);
    if !errors.is_empty() {
        let file = SourceFile::new(found, text);
        eprint!("{}", luarust_diag::report(&file, &errors));
        return Err(ExitCode::FAILURE);
    }
    Ok(project)
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
/// What one seed came to.
#[derive(Default)]
struct Tally {
    ran: u64,
    stopped: u64,
    took: u64,
}

/// Write and check one program. `Err` is the whole report, ready to print.
///
/// Everything a seed touches is one thread's: the heap, the table of array shapes, how
/// floats print, which way division rounds. That is why this can be handed to a pool
/// without a lock anywhere -- the arrangement that makes parallel *loops* in the language
/// hard is exactly what makes checking many programs at once easy.
fn one_seed(seed: u64) -> Result<Tally, String> {
    use std::fmt::Write as _;
    let mut tally = Tally::default();
    let written = luarust_gen::program(seed);
    let source = SourceFile::new(format!("seed-{seed}.lr"), written.source.clone());

    let lexed = luarust_lex::lex(source.text());
    let parsed = luarust_parse::parse(source.text(), &lexed.tokens);
    // The settings this seed runs under, so a sweep covers the combinations rather than
    // one corner of them. Every path is told the same thing, so this varies what the
    // answer is and never who agrees about it. `overflow` matters most: under `trap` the
    // JIT stops compiling arithmetic and calls back for every operation.
    let start = luarust_check::Start {
        overflow: if seed.is_multiple_of(5) {
            luarust_core::value::Overflow::Trap
        } else {
            luarust_core::value::Overflow::Wrap
        },
        collect: match (seed / 5) % 3 {
            0 => luarust_core::heap::Collect::Off,
            1 => luarust_core::heap::Collect::Silent,
            _ => luarust_core::heap::Collect::Aggressive,
        },
        floats: if (seed / 15).is_multiple_of(2) {
            luarust_core::value::Floats::Exact
        } else {
            luarust_core::value::Floats::Shortest
        },
        division: match (seed / 30) % 3 {
            0 => luarust_core::value::Division::Floored,
            1 => luarust_core::value::Division::Truncated,
            _ => luarust_core::value::Division::Euclidean,
        },
        ..luarust_check::Start::default()
    };
    let (program, errors) = luarust_check::check_with(&parsed.program, start);
    let refused: Vec<_> = lexed.errors.into_iter().chain(parsed.errors).chain(errors).collect();
    if !refused.is_empty() {
        let mut why = format!("seed {seed}: a generated program did not compile.\n\n");
        why.push_str(&written.source);
        why.push_str(&luarust_diag::report(&source, &refused));
        return Err(why);
    }

    let mut walked = Vec::new();
    let walk = luarust_interp::run(&program, &mut walked);
    let chunk = luarust_vm::compile(&program);
    let mut vm_out = Vec::new();
    let vm = luarust_vm::run(&chunk, &mut vm_out);

    let agreed = |a: &Result<(), luarust_check::value::Stopped>,
                  b: &Result<(), luarust_check::value::Stopped>| match (a, b) {
        (Ok(()), Ok(())) => true,
        (Err(a), Err(b)) => a.fault.code == b.fault.code,
        _ => false,
    };

    // And the third path, when this build has one. A program the JIT declines is not a
    // disagreement -- it is the JIT saying so, which is its right.
    #[cfg(feature = "jit")]
    if let Ok(jitted) = {
        let mut jit_out = Vec::new();
        luarust_jit::run(&chunk, &mut jit_out).map(|outcome| (outcome, jit_out))
    } {
        tally.took += 1;
        let (outcome, jit_out) = jitted;
        if walked != jit_out || !agreed(&walk, &outcome) {
            let mut why = format!("seed {seed}: the interpreter and the JIT DISAGREE.\n\n");
            why.push_str(&written.source);
            let _ = write!(why, "\ninterpreter printed:\n{}", String::from_utf8_lossy(&walked));
            let _ = write!(why, "the JIT printed:\n{}", String::from_utf8_lossy(&jit_out));
            let _ = writeln!(why, "\ninterpreter: {}", ending(&walk));
            let _ = writeln!(why, "the JIT:     {}", ending(&outcome));
            let _ = write!(why, "\n{}", chunk.disassemble());
            return Err(why);
        }
    }

    if walked != vm_out || !agreed(&walk, &vm) {
        let mut why = format!("seed {seed}: the two paths DISAGREE.\n\n");
        why.push_str(&written.source);
        let _ = write!(why, "\ninterpreter printed:\n{}", String::from_utf8_lossy(&walked));
        let _ = write!(why, "the VM printed:\n{}", String::from_utf8_lossy(&vm_out));
        let _ = writeln!(why, "\ninterpreter: {}", ending(&walk));
        let _ = writeln!(why, "the VM:      {}", ending(&vm));
        let _ = write!(why, "\n{}", chunk.disassemble());
        return Err(why);
    }

    tally.ran += 1;
    if walk.is_err() {
        tally.stopped += 1;
    }
    Ok(tally)
}

fn fuzz(count: u64) -> ExitCode {
    // Collecting hard, on purpose, and only on one of the paths. The VM sweeps its heap
    // every four kilobytes; the tree-walker and the JIT never sweep at all. A collector
    // that freed something a program could still reach would show up here as a
    // disagreement, which is a far better way to find out than a wrong answer months
    // later in something that matters.
    let hard = luarust_conf::Collect::Aggressive.threshold();
    luarust_core::heap::set_threshold(hard);

    // LLVM's target registry is process-wide and initialised on first use. Touched here,
    // once, before anything is spawned, rather than by whichever thread compiles first.
    #[cfg(feature = "jit")]
    luarust_jit::ready();

    let hands = std::thread::available_parallelism().map_or(1, |n| n.get()) as u64;
    let each = count.div_ceil(hands.max(1));

    let results = std::thread::scope(|scope| {
        let mut running = Vec::new();
        for hand in 0..hands {
            let low = 1 + hand * each;
            let high = ((hand + 1) * each).min(count);
            if low > high {
                continue;
            }
            running.push(scope.spawn(move || {
                // Per thread, because the heap is per thread.
                luarust_core::heap::set_threshold(hard);
                let mut total = Tally::default();
                for seed in low..=high {
                    match one_seed(seed) {
                        Ok(tally) => {
                            total.ran += tally.ran;
                            total.stopped += tally.stopped;
                            total.took += tally.took;
                        }
                        Err(why) => return (total, Some((seed, why))),
                    }
                }
                (total, None)
            }));
        }
        running.into_iter().map(|hand| hand.join().expect("a fuzzing thread")).collect::<Vec<_>>()
    });

    // The lowest failing seed, so which one is reported does not depend on which thread
    // finished first. A run that disagrees is a bug either way, but a bug that names a
    // different seed each time is a worse one to chase.
    let worst = results.iter().filter_map(|(_, bad)| bad.as_ref()).min_by_key(|(seed, _)| *seed);
    if let Some((_, why)) = worst {
        print!("{why}");
        return ExitCode::FAILURE;
    }

    let total = results.iter().fold(Tally::default(), |mut all, (tally, _)| {
        all.ran += tally.ran;
        all.stopped += tally.stopped;
        all.took += tally.took;
        all
    });
    let (ran, stopped) = (total.ran, total.stopped);
    #[cfg(feature = "jit")]
    println!(
        "{ran} programs on {hands} threads, all compiled, all agreed. {stopped} of them \
         stopped part way, and stopped the same way every time. The JIT took {} of them.",
        total.took
    );
    #[cfg(not(feature = "jit"))]
    println!(
        "{ran} programs on {hands} threads, all compiled, all agreed. {stopped} of them \
         stopped part way, and stopped the same way both times."
    );
    ExitCode::SUCCESS
}

fn ending(outcome: &Result<(), luarust_check::value::Stopped>) -> String {
    match outcome {
        Ok(()) => "ran to the end".to_string(),
        Err(stopped) => format!("stopped with {}", stopped.fault.code),
    }
}

/// Compile a chunk to a program that runs on its own.
///
/// Two steps and one outside tool. LLVM writes an object; the system linker joins it to
/// `luarust-native`, which is the runtime with no compiler in it. What comes out needs
/// nothing installed on the machine it lands on -- no LLVM, no `luarust`, no chunk file.
///
/// The linker is `cc`, which is the one thing this asks of the machine *building* the
/// program. That is the ordinary bargain for compiling ahead of time, and it is the
/// machine that already has a Luarust toolchain on it.
#[cfg(feature = "jit")]
fn native(
    chunk: &luarust_vm::Chunk,
    path: &Path,
    for_this_machine: bool,
    target: Option<&str>,
) -> ExitCode {
    let object = path.with_extension("o");
    if let Err(declined) = luarust_jit::write_object(chunk, &object, for_this_machine, target) {
        eprintln!("this could not be compiled ahead of time: {}", declined.because);
        return ExitCode::from(2);
    }

    let Some(archive) = runtime_archive(target) else {
        let _ = std::fs::remove_file(&object);
        match target {
            Some(name) => eprintln!(
                "the runtime archive for `{name}` was not found. Build it with\n    \
                 cargo build --release -p luarust-native --target {name}\n\
                 or name it in LUARUST_RUNTIME. A program cannot be finished for a machine \
                 whose runtime has never been built."
            ),
            None => eprintln!(
                "the runtime archive was not found. Build it with `cargo build --release \
                 -p luarust-native`, or name it in LUARUST_RUNTIME."
            ),
        }
        return ExitCode::from(2);
    };

    let out = path.with_extension("");
    let Some((linker, first)) = linker_for(target) else {
        let _ = std::fs::remove_file(&object);
        eprintln!(
            "nothing here can link for `{}`. A cross-linker is needed -- `zig cc -target \
             <triple>` is one that carries its own libc, and a `<triple>-gcc` is another.",
            target.unwrap_or("this machine")
        );
        return ExitCode::from(2);
    };
    let linked = std::process::Command::new(linker)
        .args(&first)
        .arg("-o")
        .arg(&out)
        .arg(&object)
        .arg(&archive)
        // Rust's standard library keeps a personality routine for unwinding a panic, and
        // it wants the unwinder even in a program that never panics. Named here rather
        // than left to the linker's defaults, which differ per platform.
        .args(if target.is_some() { &["-lunwind"][..] } else { &[][..] })
        .status();
    let _ = std::fs::remove_file(&object);
    match linked {
        Ok(status) if status.success() => {
            let size = std::fs::metadata(&out).map(|it| it.len()).unwrap_or(0);
            println!("{} — {size} bytes", out.display());
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("the linker refused it: {status}");
            ExitCode::from(2)
        }
        Err(why) => {
            eprintln!("`cc` could not be run: {why}. A linker is needed to finish the program.");
            ExitCode::from(2)
        }
    }
}

/// Where the runtime archive is.
///
/// Named outright in `LUARUST_RUNTIME` if the machine wants to say; otherwise beside this
/// executable, which is where a built workspace leaves it.
#[cfg(feature = "jit")]
fn runtime_archive(target: Option<&str>) -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("LUARUST_RUNTIME") {
        let named = PathBuf::from(named);
        return named.exists().then_some(named);
    }
    let here = std::env::current_exe().ok()?;
    let beside = here.parent()?;
    let found = match target {
        // Where cargo leaves it when told to build for somewhere else. The host's archive
        // would link, and would be machine code for the wrong machine.
        Some(name) => beside.join("..").join(name).join("release/libluarust_native.a"),
        None => beside.join("libluarust_native.a"),
    };
    found.exists().then_some(found)
}

/// What can turn an object into a program for `target`, and the arguments it needs first.
///
/// Building for this machine, the system's `cc` is right and needs telling nothing. For
/// somewhere else it has to be a linker that knows that somewhere -- `zig cc` carries a
/// libc for every target it supports, which is why it is looked for first; a
/// `<triple>-gcc` is the other usual answer and needs no flag because its name is the flag.
#[cfg(feature = "jit")]
fn linker_for(target: Option<&str>) -> Option<(String, Vec<String>)> {
    let Some(name) = target else {
        return Some(("cc".into(), Vec::new()));
    };
    if which("zig") {
        return Some(("zig".into(), vec!["cc".into(), "-target".into(), zigged(name)]));
    }
    let gcc = format!("{name}-gcc");
    if which(&gcc) {
        return Some((gcc, Vec::new()));
    }
    None
}

/// Zig spells a target slightly differently from Rust: no vendor in the middle.
#[cfg(feature = "jit")]
fn zigged(triple: &str) -> String {
    let parts: Vec<&str> = triple.split('-').collect();
    match parts.as_slice() {
        [arch, _vendor, system, abi] => format!("{arch}-{system}-{abi}"),
        [arch, _vendor, system] => format!("{arch}-{system}"),
        _ => triple.to_string(),
    }
}

/// Whether `PATH` has this program.
///
/// Looked for rather than run. Asking a program its version means knowing how each one
/// spells the question -- `zig version` has no dashes, and asking it for `--version` says
/// no such linker exists, which is a wrong answer arrived at confidently.
#[cfg(feature = "jit")]
fn which(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| dir.join(program).exists())
    })
}

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
            native(&chunk, &path, project.target_cpu == luarust_conf::TargetCpu::ThisMachine)
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
            Err(declined) => {
                eprintln!("the JIT declined this program: {}. Running it on the VM.", declined.because);
            }
        }
    }
    #[cfg(feature = "jit")]
    if chunk.engine == luarust_core::value::Engine::Hot {
        let mut tier = Compiling { kept: Vec::new() };
        return luarust_vm::run_with(chunk, out, Some(&mut tier));
    }
    luarust_vm::run(chunk, out)
}

/// The thing that takes a hot loop off the VM.
///
/// This lives here rather than in either crate because it is the one place that has both:
/// the VM cannot reach the JIT without closing a dependency circle, and the JIT has no
/// business driving the VM. A build with no compiler in it never constructs one.
#[cfg(feature = "jit")]
struct Compiling {
    /// What has been kept, or refused, per routine. Empty until the first loop goes hot,
    /// since a program with no hot loop should pay nothing for a cache it never fills.
    kept: Vec<Kept>,
}

/// One routine's standing with the cache.
#[cfg(feature = "jit")]
enum Kept {
    /// Never gone hot, so never compiled.
    Unasked,
    /// Compiled and held; every later call of it runs here.
    Code(luarust_jit::CompiledRoutine),
    /// The JIT declined it once, which will not change by asking again.
    Refused,
}

#[cfg(feature = "jit")]
impl luarust_vm::Tier for Compiling {
    fn hot(
        &mut self,
        chunk: &luarust_vm::Chunk,
        routine: Option<usize>,
        at: usize,
        frames: Vec<Vec<luarust_core::value::Value>>,
        started: std::time::Instant,
        out: &mut dyn std::io::Write,
    ) -> luarust_vm::Taken {
        // A routine that went hot once will be called again — that is what hot means —
        // so it is compiled for keeps as well as for this activation. The activation
        // that tripped the counter is mid-loop and needs the resumed shape; every call
        // after it enters at the top, on the kept one.
        if let Some(index) = routine {
            if self.kept.is_empty() {
                self.kept.resize_with(chunk.funcs.len(), || Kept::Unasked);
            }
            if matches!(self.kept[index], Kept::Unasked) {
                self.kept[index] = match luarust_jit::compile_routine(chunk, index) {
                    Ok(code) => Kept::Code(code),
                    Err(declined) => {
                        eprintln!(
                            "the JIT declined to keep this routine: {}. Its calls stay \
                             on the VM.",
                            declined.because
                        );
                        Kept::Refused
                    }
                };
            }
        }
        let taken = match routine {
            None => luarust_jit::resume(chunk, at, frames, started, out)
                .map(luarust_vm::Taken::Finished),
            Some(index) => luarust_jit::resume_routine(chunk, index, at, frames, started, out)
                .map(luarust_vm::Taken::Returned),
        };
        match taken {
            Ok(taken) => taken,
            Err(declined) => {
                eprintln!(
                    "the JIT declined this loop: {}. Carrying on with the VM.",
                    declined.because
                );
                luarust_vm::Taken::Declined
            }
        }
    }

    fn keeps(&self, routine: usize) -> bool {
        matches!(self.kept.get(routine), Some(Kept::Code(_)))
    }

    fn call(
        &mut self,
        _chunk: &luarust_vm::Chunk,
        routine: usize,
        open: &[&Vec<luarust_core::value::Value>],
        fresh: Vec<luarust_core::value::Value>,
        started: std::time::Instant,
        out: &mut dyn std::io::Write,
    ) -> Result<Option<luarust_core::value::Value>, luarust_core::value::Stopped> {
        let Kept::Code(code) = &self.kept[routine] else {
            unreachable!("only asked about a routine `keeps` said yes to");
        };
        code.call(open, fresh, started, out)
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
fn fuzz(count: u64) -> ExitCode {
    // Collecting hard, on purpose, and only on one of the paths. The VM sweeps its heap
    // every four kilobytes; the tree-walker and the JIT never sweep at all. A collector
    // that freed something a program could still reach would show up here as a
    // disagreement, which is a far better way to find out than a wrong answer months
    // later in something that matters.
    luarust_core::heap::set_threshold(luarust_conf::Collect::Aggressive.threshold());

    let mut ran = 0u64;
    #[cfg(feature = "jit")]
    let mut took = 0u64;
    let mut stopped = 0u64;

    for seed in 1..=count {
        let written = luarust_gen::program(seed);
        let source = SourceFile::new(format!("seed-{seed}.lr"), written.source.clone());

        let lexed = luarust_lex::lex(source.text());
        let parsed = luarust_parse::parse(source.text(), &lexed.tokens);
        // The settings this seed runs under, so a sweep covers the combinations rather
        // than one corner of them. Every path is told the same thing, so this varies what
        // the answer is and never who agrees about it. `overflow` matters most: under
        // `trap` the JIT stops compiling arithmetic and calls back for every operation.
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
        // And the third path, when this build has one. A program the JIT declines is not a
        // disagreement -- it is the JIT saying so, which is its right.
        #[cfg(feature = "jit")]
        if let Ok(jitted) = {
            let mut jit_out = Vec::new();
            luarust_jit::run(&chunk, &mut jit_out).map(|outcome| (outcome, jit_out))
        } {
            took += 1;
            let (outcome, jit_out) = jitted;
            let same_ending = match (&walk, &outcome) {
                (Ok(()), Ok(())) => true,
                (Err(a), Err(b)) => a.fault.code == b.fault.code,
                _ => false,
            };
            if walked != jit_out || !same_ending {
                println!("seed {seed}: the interpreter and the JIT DISAGREE.\n");
                print!("{}", written.source);
                println!("\ninterpreter printed:\n{}", String::from_utf8_lossy(&walked));
                println!("the JIT printed:\n{}", String::from_utf8_lossy(&jit_out));
                println!("\ninterpreter: {}", ending(&walk));
                println!("the JIT:     {}", ending(&outcome));
                println!("\n{}", chunk.disassemble());
                return ExitCode::FAILURE;
            }
        }

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

    #[cfg(feature = "jit")]
    println!(
        "{ran} programs, all compiled, all agreed. {stopped} of them stopped part way, \
         and stopped the same way every time. The JIT took {took} of them."
    );
    #[cfg(not(feature = "jit"))]
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
fn native(chunk: &luarust_vm::Chunk, path: &Path, for_this_machine: bool) -> ExitCode {
    let object = path.with_extension("o");
    if let Err(declined) = luarust_jit::write_object(chunk, &object, for_this_machine) {
        eprintln!("this could not be compiled ahead of time: {}", declined.because);
        return ExitCode::from(2);
    }

    let Some(archive) = runtime_archive() else {
        eprintln!(
            "the runtime archive was not found. Build it with `cargo build --release -p \
             luarust-native`, or name it in LUARUST_RUNTIME."
        );
        return ExitCode::from(2);
    };

    let out = path.with_extension("");
    let linked = std::process::Command::new("cc")
        .arg("-o")
        .arg(&out)
        .arg(&object)
        .arg(&archive)
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
fn runtime_archive() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("LUARUST_RUNTIME") {
        let named = PathBuf::from(named);
        return named.exists().then_some(named);
    }
    let here = std::env::current_exe().ok()?;
    let beside = here.parent()?.join("libluarust_native.a");
    beside.exists().then_some(beside)
}

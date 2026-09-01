//! Generated programs, run both ways, made to agree.
//!
//! This is the oracle doing the job it was built for. The tests written by hand cover the
//! cases somebody thought of; these cover the ones nobody did.

use luarust_check::Start;
use luarust_core::heap::Collect;
use luarust_core::value::{Division, Floats, Overflow};
use luarust_diag::SourceFile;

/// The project settings this seed runs under.
///
/// A generated program is run under one set of settings at a time and the seed picks
/// which, so a sweep covers the combinations rather than one corner of them. Every path
/// is told the same thing, so this varies what the answer should be and never who agrees
/// about it.
///
/// `overflow` is the one that changes most: under `trap` the JIT stops compiling
/// arithmetic and calls back into the runtime for every operation, which is a different
/// body of machine code entirely and was never once fuzzed while this said `wrap`.
fn settings_for(seed: u64) -> Start {
    Start {
        overflow: if seed.is_multiple_of(5) { Overflow::Trap } else { Overflow::Wrap },
        collect: match (seed / 5) % 3 {
            0 => Collect::Off,
            1 => Collect::Silent,
            _ => Collect::Aggressive,
        },
        floats: if (seed / 15).is_multiple_of(2) { Floats::Exact } else { Floats::Shortest },
        division: match (seed / 30) % 3 {
            0 => Division::Floored,
            1 => Division::Truncated,
            _ => Division::Euclidean,
        },
        ..Start::default()
    }
}


enum Outcome {
    Printed(String),
    Stopped(&'static str),
}

/// Run one generated program both ways and insist the two agree about everything: what
/// they printed, and whether and why they stopped.
fn agree(source: &str, seed: u64) -> Outcome {
    let file = SourceFile::new("generated.lr", source);
    let complain = |what: &str, errors: &[luarust_diag::Diagnostic]| -> ! {
        panic!(
            "seed {seed}: a generated program should always {what}\n\n{source}\n{}",
            luarust_diag::report(&file, errors)
        )
    };

    let lexed = luarust_lex::lex(source);
    if !lexed.ok() {
        complain("lex", &lexed.errors);
    }
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    if !parsed.ok() {
        complain("parse", &parsed.errors);
    }
    let (program, errors) = luarust_check::check_with(
        &parsed.program,
        settings_for(seed),
    );
    if !errors.is_empty() {
        complain("check", &errors);
    }

    let mut walked = Vec::new();
    let walk = luarust_interp::run(&program, &mut walked);
    let chunk = luarust_vm::compile(&program);
    let mut ran = Vec::new();
    let vm = luarust_vm::run(&chunk, &mut ran);

    let walked = String::from_utf8_lossy(&walked).into_owned();
    let ran = String::from_utf8_lossy(&ran).into_owned();

    assert_eq!(
        walked, ran,
        "seed {seed}: the two paths printed different things\n\n{source}\n{}",
        chunk.disassemble()
    );

    match (walk, vm) {
        (Ok(()), Ok(())) => Outcome::Printed(walked),
        (Err(a), Err(b)) => {
            assert_eq!(
                a.fault.code, b.fault.code,
                "seed {seed}: they stopped for different reasons\n\n{source}"
            );
            Outcome::Stopped(a.fault.code)
        }
        (Ok(()), Err(b)) => panic!(
            "seed {seed}: the VM stopped and the interpreter did not: {}\n\n{source}\n{}",
            b.fault.message,
            chunk.disassemble()
        ),
        (Err(a), Ok(())) => panic!(
            "seed {seed}: the interpreter stopped and the VM did not: {}\n\n{source}",
            a.fault.message
        ),
    }
}

#[test]
fn a_generated_program_always_compiles_and_the_two_paths_always_agree() {
    let mut printed = 0;
    let mut stopped = 0;
    let mut faults: std::collections::BTreeSet<&str> = Default::default();
    for seed in 1..=2000 {
        match agree(&luarust_gen::program(seed).source, seed) {
            Outcome::Printed(text) => {
                // Every generated program ends with a print, so one that ran to the end
                // and printed nothing means the generator has stopped generating.
                assert!(!text.is_empty(), "seed {seed}: ran to the end and printed nothing");
                printed += 1;
            }
            Outcome::Stopped(code) => {
                faults.insert(code);
                stopped += 1;
            }
        }
    }
    assert!(faults.len() > 1, "only ever hit one kind of fault: {faults:?}");
    // Both kinds should turn up. Programs that only ever run to the end would mean the
    // faults are never being compared, and programs that only ever stop would mean
    // nothing is being computed.
    assert!(printed > 1500, "only {printed} of 2000 ran to the end");
    assert!(stopped > 0, "none of 2000 ever stopped, so faults are going unchecked");
}

#[test]
fn a_seed_always_writes_the_same_program() {
    // So that a failure can be looked at again by name.
    for seed in [1, 42, 1000] {
        assert_eq!(luarust_gen::program(seed).source, luarust_gen::program(seed).source);
    }
    assert_ne!(luarust_gen::program(1).source, luarust_gen::program(2).source);
}

#[test]
fn the_programs_are_worth_running() {
    // A generator that writes nothing but `print` would pass every agreement test and
    // discover nothing, so check that the interesting constructs actually appear.
    let all: String = (1..=800).map(|seed| luarust_gen::program(seed).source).collect();
    for construct in [
        "loop.temp", "loop.perm", "handback", "set [", "math {", " mod ", " div ", "**",
        // Branching, and the three words that join conditions.
        "if [", "else-if [", "} else {", " and ", " or ", "not (",
        // Functions: declared, answering something and nothing, called for a value and
        // called for what they do, and one that calls itself.
        "fn.local.", "fn.local.nothing ", "return ", "return;", "f0[",
        // Loops that stop on a condition, and both ways of leaving one early.
        "loop.temp.while.", "loop.perm.while.", "break;", "break when reached ",
        // The comparisons -- every spelling of every one, since a spelling that is never
        // written is a spelling nobody ever finds out is broken -- and the two types that
        // are not numbers.
        " < ", " > ", " = ", " != ", " not= ", " ≠ ",
        " </= ", " <= ", " ≤ ", " >/= ", " >= ", " ≥ ", "bool", "str",
        // A literal that says what it is, and one that takes its type from context.
        "i32 |", "b64 |", "= [|",
    ] {
        assert!(all.contains(construct), "the generator never writes `{construct}`");
    }
    for ty in ["b16", "b256", "i8", "ui64", "er", "d32", "d64", "d128"] {
        assert!(all.contains(ty), "the generator never uses `{ty}`");
    }
}



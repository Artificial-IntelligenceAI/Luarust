//! A one-off deep sweep, not part of the ordinary gate: 200,000 generated programs,
//! three ways each. Run it with `--ignored` when a change touches what the JIT emits.

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


fn three_ways(source: &str, seed: u64) {
    let file = SourceFile::new("test.lr", source);
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{}", luarust_diag::report(&file, &lexed.errors));
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{}", luarust_diag::report(&file, &parsed.errors));
    let (program, errors) = luarust_check::check_with(
        &parsed.program,
        settings_for(seed),
    );
    assert!(errors.is_empty(), "{}", luarust_diag::report(&file, &errors));

    let mut walked = Vec::new();
    let walk = luarust_interp::run(&program, &mut walked);
    let chunk = luarust_vm::compile(&program);
    let mut ran = Vec::new();
    let vm = luarust_vm::run(&chunk, &mut ran);
    assert_eq!(
        String::from_utf8_lossy(&walked),
        String::from_utf8_lossy(&ran),
        "the interpreter and the VM disagree\n\n{source}"
    );
    assert_eq!(
        walk.as_ref().err().map(|s| s.fault.code),
        vm.as_ref().err().map(|s| s.fault.code),
        "the interpreter and the VM end differently\n\n{source}"
    );
    let mut compiled = Vec::new();
    let jit = luarust_jit::run(&chunk, &mut compiled).expect("the JIT takes everything now");
    assert_eq!(
        String::from_utf8_lossy(&walked),
        String::from_utf8_lossy(&compiled),
        "the compiled program printed something else\n\n{source}"
    );
    match (walk, jit) {
        (Ok(()), Ok(())) => {}
        (Err(a), Err(b)) => assert_eq!(
            a.fault.code, b.fault.code,
            "they stopped for different reasons\n\n{source}"
        ),
        (a, b) => panic!("one stopped and the other did not: {a:?} vs {b:?}\n\n{source}"),
    }
}

#[test]
#[ignore = "a deep sweep for changes to what the JIT emits, not for every gate"]
fn two_hundred_thousand_agree_three_ways() {
    for seed in 1..=200_000u64 {
        three_ways(&luarust_gen::program(seed).source, seed);
    }
}

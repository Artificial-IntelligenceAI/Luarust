#![cfg(feature = "compile")]
//! A chunk has to end in something that stops.
//!
//! Every jump is held against the instruction count, so control can only leave an
//! instruction for a real one — except by walking off the last, which nothing held
//! against anything. `serialize` promises in its own first paragraph that a corrupt file
//! produces a complaint and not a crash, and four instructions were enough to break it.

use luarust_vm::chunk::{Chunk, Op, Routine};
use luarust_vm::serialize::{self, Broken};

fn chunk_of(source: &str) -> Option<Chunk> {
    let lexed = luarust_lex::lex(source);
    if !lexed.ok() {
        return None;
    }
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    if !parsed.ok() {
        return None;
    }
    let (program, errors) = luarust_check::check(&parsed.program);
    if !errors.is_empty() {
        return None;
    }
    Some(luarust_vm::compile(&program))
}

/// The chunk that found this: give a register a type, jump over the halt, and land on an
/// instruction that falls through, with nothing after it. Before the rule it passed every
/// check, loaded without complaint, and then panicked reading instruction four of four.
fn runs_off_the_end() -> Chunk {
    Chunk {
        code: vec![
            Op::Const { dst: 0, konst: 0 },
            Op::Jump { target: 3 },
            Op::Halt,
            Op::Move { dst: 0, src: 0, ty: luarust_core::Ty::U32 },
        ],
        spans: vec![luarust_diag::Span::default(); 4],
        consts: vec![luarust_core::value::Value::Num { ty: luarust_core::Ty::U32, bits: 0 }],
        texts: Vec::new(),
        registers: 1,
        overflow: Default::default(),
        collect: Default::default(),
        floats: Default::default(),
        engine: Default::default(),
        division: Default::default(),
        funcs: Vec::new(),
    }
}

#[test]
fn a_chunk_that_would_run_off_the_end_is_refused() {
    let bytes = serialize::write(&runs_off_the_end(), "x.lr", "");
    match serialize::read(&bytes) {
        Err(Broken::NeverStops { what, .. }) => assert_eq!(what, "top level"),
        Err(other) => panic!("refused, but for the wrong reason: {other}"),
        Ok(_) => panic!("loaded a chunk that runs off the end of its own code"),
    }
}

#[test]
fn a_routine_that_would_run_off_the_end_is_refused() {
    let mut chunk = runs_off_the_end();
    chunk.code = vec![Op::Halt];
    chunk.spans = vec![luarust_diag::Span::default()];
    chunk.funcs = vec![Routine {
        code: vec![Op::Const { dst: 0, konst: 0 }],
        spans: vec![luarust_diag::Span::default()],
        registers: 1,
        params: Vec::new(),
        returns: None,
    }];
    let bytes = serialize::write(&chunk, "x.lr", "");
    match serialize::read(&bytes) {
        Err(Broken::NeverStops { what, .. }) => assert_eq!(what, "a routine"),
        Err(other) => panic!("refused, but for the wrong reason: {other}"),
        Ok(_) => panic!("loaded a routine that runs off the end of its own code"),
    }
}

/// The other half of a rule: that it refuses nothing anybody meant. Five thousand
/// generated programs and every example in the tree, compiled and then loaded back.
#[test]
fn nothing_the_compiler_writes_is_refused_by_it() {
    let mut sources: Vec<String> =
        (1..=5_000u64).map(|seed| luarust_gen::program(seed).source).collect();
    let examples = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");
    for entry in std::fs::read_dir(examples).expect("the examples are in the tree") {
        let path = entry.expect("a readable entry").path();
        if path.extension().is_some_and(|kind| kind == "lr") {
            sources.push(std::fs::read_to_string(&path).expect("a readable example"));
        }
    }

    let (mut chunks, mut routines) = (0, 0);
    for source in &sources {
        let Some(chunk) = chunk_of(source) else { continue };
        chunks += 1;
        routines += chunk.funcs.len();
        assert!(matches!(chunk.code.last(), Some(Op::Halt)), "the top level does not halt");
        for (index, routine) in chunk.funcs.iter().enumerate() {
            assert!(
                matches!(routine.code.last(), Some(Op::Return { .. } | Op::ReturnNothing)),
                "f{index} does not return"
            );
        }
        let bytes = serialize::write(&chunk, "test.lr", source);
        if let Err(broken) = serialize::read(&bytes) {
            panic!("a chunk the compiler wrote was refused: {broken}");
        }
    }
    assert!(chunks > 4_000, "only {chunks} compiled, which proves too little");
    println!("{chunks} chunks and {routines} routines, all ending in a stop");
}

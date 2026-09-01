//! A one-off deep sweep, not part of the ordinary gate: 200,000 generated programs,
//! three ways each. Run it with `--ignored` when a change touches what the JIT emits.

use luarust_diag::SourceFile;

fn three_ways(source: &str) {
    let file = SourceFile::new("test.lr", source);
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{}", luarust_diag::report(&file, &lexed.errors));
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{}", luarust_diag::report(&file, &parsed.errors));
    let (program, errors) = luarust_check::check(&parsed.program);
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
        three_ways(&luarust_gen::program(seed).source);
    }
}

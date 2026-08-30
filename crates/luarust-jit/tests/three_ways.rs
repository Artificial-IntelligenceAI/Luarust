//! The same program, three ways, insisting on one answer.
//!
//! The interpreter, the bytecode VM, and machine code made by LLVM. Where the JIT declines
//! a program there are two answers to compare instead of three, which is still worth
//! having; where it takes one there are three, and three implementations that must agree
//! is a much harder thing to satisfy by accident than two.

use luarust_diag::SourceFile;

enum Ran {
    All(String),
    Declined(String),
}

fn three_ways(source: &str) -> Ran {
    let file = SourceFile::new("test.lr", source);
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{}", luarust_diag::report(&file, &lexed.errors));
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{}", luarust_diag::report(&file, &parsed.errors));
    let (program, errors) = luarust_check::check(&parsed.program);
    assert!(errors.is_empty(), "{}", luarust_diag::report(&file, &errors));

    let mut walked = Vec::new();
    let walk = luarust_interp::run(&program, &mut walked);
    let mut ran = Vec::new();
    let vm = luarust_vm::run(&luarust_vm::compile(&program), &mut ran);
    assert_eq!(
        String::from_utf8_lossy(&walked),
        String::from_utf8_lossy(&ran),
        "the interpreter and the VM already disagree"
    );
    assert_eq!(
        walk.as_ref().err().map(|s| s.fault.code),
        vm.as_ref().err().map(|s| s.fault.code),
        "the interpreter and the VM already end differently"
    );

    let mut compiled = Vec::new();
    match luarust_jit::run(&program, &mut compiled) {
        Err(declined) => Ran::Declined(declined.because),
        Ok(jit) => {
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
            Ran::All(String::from_utf8_lossy(&compiled).into_owned())
        }
    }
}

fn ran(source: &str) -> String {
    match three_ways(source) {
        Ran::All(out) => out,
        Ran::Declined(why) => panic!("expected the JIT to take this: {why}\n\n{source}"),
    }
}

#[test]
fn counting() {
    assert_eq!(ran("loop.temp.range.ui8 ['i'] = ['1', '5'] { print['i' \\n]; }"), "1\n2\n3\n4\n5\n");
}

#[test]
fn accumulating() {
    assert_eq!(
        ran("var.local.mut.ui32 ['total'] = ['0'];\n\
             loop.temp.range.ui32 ['i'] = ['1', '10'] { handback 'i' as 'total'; }\n\
             print[\"total is \" 'total' \\n];"),
        "total is 55\n"
    );
}

#[test]
fn the_benchmark() {
    assert_eq!(
        ran("var.local.mut.ui64 ['sum'] = ['0'];\n\
             loop.temp.range.ui64 ['i'] = ['1', '100000'] {\n\
                 set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];\n\
             }\n\
             print['sum'];"),
        "49965"
    );
}

#[test]
fn the_edges_of_a_range() {
    assert_eq!(ran("loop.temp.range.ui8 ['i'] = ['3', '3'] { print['i']; }"), "3");
    assert_eq!(ran("loop.temp.range.ui8 ['i'] = ['5', '1'] { print['i']; }"), "");
    assert_eq!(ran("loop.temp.range.ui8 ['i'] = ['253','255'] { print['i' \" \"]; }"), "253 254 255 ");
    assert_eq!(ran("loop.perm.range.ui8 ['i'] = ['1', '5'] { } print['i'];"), "5");
}

#[test]
fn nested_loops() {
    assert_eq!(
        ran("loop.temp.range.ui8 ['a'] = ['1','3'] {\n\
                 loop.temp.range.ui8 ['b'] = ['1','3'] { print['a' 'b' \" \"]; }\n\
             }"),
        "11 12 13 21 22 23 31 32 33 "
    );
}

#[test]
fn raising_to_a_power_is_taken_now() {
    // Emitted as a call back into luarust-num rather than as instructions, because IEEE
    // does not require `pow` to be correctly rounded and there is no argument that the
    // hardware and the software would agree.
    let math = |e: &str| ran(&format!("var.local.i32 ['r'] = [math {{ {e} }}]; print['r'];"));
    assert_eq!(math("2 ** 3"), "8");
    assert_eq!(math("2 ** 3 ** 2"), "512");
    assert_eq!(math("-2 ** 2"), "-4");
    assert_eq!(
        ran("var.local.b64 ['r'] = [math { 1.5 ** 3 }]; print['r'];"),
        "3.375"
    );
}

#[test]
fn integer_arithmetic() {
    let math = |e: &str| ran(&format!("var.local.i32 ['r'] = [math {{ {e} }}]; print['r'];"));
    assert_eq!(math("2 + 3 * 4"), "14");
    assert_eq!(math("(2 + 3) * 4"), "20");
    assert_eq!(math("-7 mod 3"), "2");
    assert_eq!(math("7 mod -3"), "-2");
    assert_eq!(math("100 div 5 div 2"), "10");
    assert_eq!(math("-2 - 3"), "-5");
}

#[test]
fn float_arithmetic_including_the_signs_of_zero() {
    let b64 = |e: &str| ran(&format!("var.local.b64 ['r'] = [math {{ {e} }}]; print['r'];"));
    assert_eq!(b64("1 div 4"), "0.25");
    assert_eq!(b64("0.1 + 0.2"), "0.30000000000000004");
    assert_eq!(b64("1 div 0"), "inf");
    assert_eq!(b64("0 div 0"), "nan");
    // The one that `0 - x` would have got wrong.
    assert_eq!(b64("-0"), "-0");
    assert_eq!(b64("0 - 0"), "0");
    assert_eq!(ran("var.local.b32 ['r'] = [math { 1 div 3 }]; print['r'];"), "0.3333333432674408");
}

#[test]
fn a_program_that_stops_stops_the_same_way() {
    three_ways("print[\"before\"]; var.local.i32 ['x'] = [math { 1 div 0 }];");
    three_ways("defaults.overflow.trap; var.local.ui8 ['x'] = [math { 255 + 1 }];");
}

#[test]
fn what_it_will_not_take_it_says_so_about() {
    for (source, expected) in [
        ("var.local.b16 ['x'] = ['1']; print['x'];", "b16"),
        ("var.local.b128 ['x'] = ['1']; print['x'];", "b128"),
        ("var.local.b256 ['x'] = ['1']; print['x'];", "b256"),
        ("var.local.str ['x'] = ['hi']; print['x'];", "str"),
    ] {
        match three_ways(source) {
            Ran::Declined(why) => assert!(why.contains(expected), "got `{why}`, wanted `{expected}`"),
            Ran::All(_) => panic!("the JIT took something it should have declined: {source}"),
        }
    }
}

#[test]
fn generated_programs_agree_three_ways() {
    let mut taken = 0;
    let mut declined = 0;
    for seed in 1..=3000 {
        match three_ways(&luarust_gen::program(seed).source) {
            Ran::All(_) => taken += 1,
            Ran::Declined(_) => declined += 1,
        }
    }
    // Printed so a change in what the JIT will take shows up as a number rather than as
    // a feeling.
    println!("the JIT took {taken} of 3000 and declined {declined}");
    assert!(taken > 500, "the JIT only took {taken} of 3000, which is too few to prove much");
    assert!(declined > 0, "it took all 3000, so the declining is not being exercised");
}

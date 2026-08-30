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
    assert_eq!(ran("loop.temp.range.ui8 ['i'] = [|1|, |5|] { print['i' \\n]; }"), "1\n2\n3\n4\n5\n");
}

#[test]
fn accumulating() {
    assert_eq!(
        ran("var.local.mut.ui32 ['total'] = [|0|];\n\
             loop.temp.range.ui32 ['i'] = [|1|, |10|] { handback 'i' as 'total'; }\n\
             print[\"total is \" 'total' \\n];"),
        "total is 55\n"
    );
}

#[test]
fn the_benchmark() {
    assert_eq!(
        ran("var.local.mut.ui64 ['sum'] = [|0|];\n\
             loop.temp.range.ui64 ['i'] = [|1|, |100000|] {\n\
                 set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];\n\
             }\n\
             print['sum'];"),
        "49965"
    );
}

#[test]
fn the_edges_of_a_range() {
    assert_eq!(ran("loop.temp.range.ui8 ['i'] = [|3|, |3|] { print['i']; }"), "3");
    assert_eq!(ran("loop.temp.range.ui8 ['i'] = [|5|, |1|] { print['i']; }"), "");
    assert_eq!(ran("loop.temp.range.ui8 ['i'] = [|253|,|255|] { print['i' \" \"]; }"), "253 254 255 ");
    assert_eq!(ran("loop.perm.range.ui8 ['i'] = [|1|, |5|] { } print['i'];"), "5");
}

#[test]
fn nested_loops() {
    assert_eq!(
        ran("loop.temp.range.ui8 ['a'] = [|1|,|3|] {\n\
                 loop.temp.range.ui8 ['b'] = [|1|,|3|] { print['a' 'b' \" \"]; }\n\
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
fn b16_is_taken_now_even_though_it_has_no_instructions() {
    // Carried as its sixteen-bit encoding, worked on by calling back into luarust-num.
    // The README's own number, which is what `b16 '0.1'` actually is.
    assert_eq!(ran("var.local.b16 ['a'] = [|0.1|]; print['a'];"), "0.0999755859375");
    assert_eq!(
        ran("var.local.b16 ['a','b'] = [|1.5|,|0.25|]; var.local.b16 ['c'] = [math { 'a' + 'b' }]; print['c'];"),
        "1.75"
    );
    // Negating flips the sign bit, so a zero keeps a sign the way it does everywhere else.
    assert_eq!(ran("var.local.b16 ['z'] = [math { -0 }]; print['z'];"), "-0");
    // And ordering, which is neither an integer nor a float comparison at this width.
    assert_eq!(
        ran("loop.temp.range.b16 ['i'] = [|1|, |4|] { print['i' \" \"]; }"),
        "1 2 3 4 "
    );
    // The remainder, which goes back to luarust-num rather than to the hardware.
    assert_eq!(ran("var.local.b16 ['r'] = [math { -7 mod 3 }]; print['r'];"), "2");
}

#[test]
fn the_clock_is_read_as_the_type_it_was_asked_for() {
    // Not compared across paths: they run at different speeds, so they read different
    // times, and that is the clock working rather than the paths disagreeing. What is
    // checked is that the number means seconds -- answering in b64 whatever was asked for
    // would put a b64's bits in a b32 variable and the value would be nonsense.
    for ty in ["b32", "b64"] {
        let source = format!("var.local.{ty} ['t'] = [time.now]; print['t'];");
        let lexed = luarust_lex::lex(&source);
        let parsed = luarust_parse::parse(&source, &lexed.tokens);
        let (program, errors) = luarust_check::check(&parsed.program);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut out = Vec::new();
        luarust_jit::run(&program, &mut out).expect("the JIT took it").expect("it ran");
        let text = String::from_utf8(out).expect("output is text");
        let seconds: f64 = text.parse().unwrap_or_else(|_| panic!("{ty} gave `{text}`"));
        assert!((0.0..60.0).contains(&seconds), "{ty} said {seconds} seconds");
    }
}

#[test]
fn a_program_that_stops_stops_the_same_way() {
    three_ways("print[\"before\"]; var.local.i32 ['x'] = [math { 1 div 0 }];");
    three_ways("defaults.overflow.trap; var.local.ui8 ['x'] = [math { 255 + 1 }];");
}

#[test]
fn the_types_with_no_instructions_are_taken_too() {
    // They live in numbered cells on the Rust side and everything done to them is a call,
    // which is what their arithmetic always was. Taken for the coverage, not the speed.
    assert_eq!(ran("var.local.b128 ['x'] = [|0.1|]; print['x'];"), "0.1");
    assert_eq!(ran("var.local.b256 ['x'] = [|0.1|]; print['x'];"), "0.1");
    assert_eq!(
        ran("var.local.b256 ['a','b'] = [|1.5|,|0.25|]; var.local.b256 ['c'] = [math { 'a' + 'b' }]; print['c'];"),
        "1.75"
    );
    assert_eq!(ran("var.local.b128 ['r'] = [math { -7 mod 3 }]; print['r'];"), "2");
    // Ordering, which is a call as well.
    assert_eq!(
        ran("loop.temp.range.b128 ['i'] = [|1|, |4|] { print['i' \" \"]; }"),
        "1 2 3 4 "
    );
}

#[test]
fn text_and_truth_are_taken_too() {
    assert_eq!(ran("var.local.str ['who'] = [|🧑‍🧑‍🧒‍🧒|]; print['who'];"), "🧑‍🧑‍🧒‍🧒");
    assert_eq!(
        ran("var.local.mut.str ['s'] = [|first|]; set ['s'] = [|second|]; print['s'];"),
        "second"
    );
    assert_eq!(ran("var.local.bool ['t'] = [|true|]; print['t'];"), "true");
    assert_eq!(
        ran("var.local.str ['a','b'] = [|one|,|two|]; print['a' \" and \" 'b' \\n];"),
        "one and two\n"
    );
}

#[test]
fn a_program_mixing_everything_agrees_three_ways() {
    assert_eq!(
        ran("var.local.str ['who'] = [|world|];\n\
             var.local.mut.i64 ['sum'] = [|0|];\n\
             var.local.b16 ['small'] = [|0.1|];\n\
             var.local.b256 ['wide'] = [|0.1|];\n\
             var.local.bool ['yes'] = [|true|];\n\
             loop.temp.range.i64 ['i'] = [|1|, |5|] { handback 'i' as 'sum'; }\n\
             print[\"hello \" 'who' \" \" 'sum' \" \" 'small' \" \" 'wide' \" \" 'yes' \\n];"),
        "hello world 15 0.0999755859375 0.1 true\n"
    );
}

#[test]
fn an_if_runs_exactly_one_arm() {
    let chain = "var.local.i32 ['n'] = [|@|];\n\
         if [math { 'n' > i32 |10| }] { print[\"big\" \\n]; }\n\
         else-if [math { 'n' = i32 |10| }] { print[\"ten\" \\n]; }\n\
         else { print[\"small\" \\n]; }";
    assert_eq!(ran(&chain.replace('@', "12")), "big\n");
    assert_eq!(ran(&chain.replace('@', "10")), "ten\n");
    assert_eq!(ran(&chain.replace('@', "3")), "small\n");
}

#[test]
fn an_if_with_no_else_and_nothing_true_does_nothing() {
    assert_eq!(
        ran("var.local.i32 ['n'] = [|1|];\n\
             print[\"before \"];\n\
             if [math { 'n' > i32 |10| }] { print[\"never\"]; }\n\
             print[\"after\" \\n];"),
        "before after\n"
    );
}

#[test]
fn and_or_and_not_answer_the_way_a_table_says_they_should() {
    // Every row of both truth tables, and both ways of writing each.
    let mut out = String::new();
    for a in ["true", "false"] {
        for b in ["true", "false"] {
            out.push_str(&format!(
                "print[math {{ bool |{a}| and bool |{b}| }} \" \" math {{ bool |{a}| or bool |{b}| }} \\n];\n"
            ));
        }
    }
    out.push_str("print[math { not bool |true| } \" \" math { not bool |false| } \\n];");
    assert_eq!(
        ran(&out),
        "true true\nfalse true\nfalse true\nfalse false\nfalse true\n"
    );
}

#[test]
fn a_condition_can_guard_the_one_after_it() {
    // The right side of an `and` is a fault when the left is false. If it were worked out
    // anyway this would stop instead of printing, on every one of the three paths.
    assert_eq!(
        ran("var.local.i32 ['n'] = [|7|];\n\
             var.local.i32 ['d'] = [|0|];\n\
             if [math { 'd' != i32 |0| and 'n' div 'd' > i32 |1| }] { print[\"divided\" \\n]; }\n\
             else { print[\"guarded\" \\n]; }"),
        "guarded\n"
    );
}

#[test]
fn the_answer_survives_being_written_where_it_came_from() {
    // `set ['f'] = [math { … 'f' … }]` reads `f` after the first half has already been
    // worked out. Building that half where `f` lives would lose it.
    assert_eq!(
        ran("var.local.mut.bool ['f'] = [|true|];\n\
             set ['f'] = [math { ('f' and (i32 |1| = i32 |2|)) or 'f' }];\n\
             print['f' \\n];"),
        "true\n"
    );
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
    // a feeling. It takes all of them now, so every generated program is checked three
    // ways rather than two -- which is the whole reason for taking the types it cannot
    // compute with.
    println!("the JIT took {taken} of 3000 and declined {declined}");
    assert_eq!(declined, 0, "the JIT declined {declined} programs");
    assert_eq!(taken, 3000);
}

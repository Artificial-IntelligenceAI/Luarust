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
    let chunk = luarust_vm::compile(&program);
    let mut ran = Vec::new();
    let vm = luarust_vm::run(&chunk, &mut ran);
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

    // The JIT is given the same chunk the VM just ran, not the tree it came from. That
    // is the whole point of it reading bytecode: one artefact, three ways to run it.
    let mut compiled = Vec::new();
    match luarust_jit::run(&chunk, &mut compiled) {
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
    assert_eq!(b64("0.1 + 0.2"), "0.3000000000000000444089209850062616169452667236328125");
    assert_eq!(b64("1 div 0"), "inf");
    assert_eq!(b64("0 div 0"), "nan");
    // The one that `0 - x` would have got wrong.
    assert_eq!(b64("-0"), "-0");
    assert_eq!(b64("0 - 0"), "0");
    assert_eq!(ran("var.local.b32 ['r'] = [math { 1 div 3 }]; print['r'];"), "0.3333333432674407958984375");
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
        luarust_jit::run(&luarust_vm::compile(&program), &mut out)
            .expect("the JIT took it")
            .expect("it ran");
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
    assert_eq!(ran("var.local.b128 ['x'] = [|0.1|]; print['x'];"), "0.1000000000000000000000000000000000048148248609680896326399448564623182963452541205384704880998469889163970947265625");
    assert_eq!(ran("var.local.b256 ['x'] = [|0.1|]; print['x'];"), "0.10000000000000000000000000000000000000000000000000000000000000000000000022639197697066780918772798227219479451706327995347845473956537224838753296482112786848828629902395248634881098242194574139706832183183138340609730221331119537353515625");
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
        "hello world 15 0.0999755859375 0.10000000000000000000000000000000000000000000000000000000000000000000000022639197697066780918772798227219479451706327995347845473956537224838753296482112786848828629902395248634881098242194574139706832183183138340609730221331119537353515625 true\n"
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
fn a_function_answers_the_same_thing_three_ways() {
    assert_eq!(
        ran("fn.local.i32 ['bigger'] [i32 'a', i32 'b'] {\n\
                 if [math { 'a' > 'b' }] { return 'a'; }\n\
                 return 'b';\n\
             }\n\
             print[bigger[|3|, |9|] \\n];"),
        "9\n"
    );
}

#[test]
fn recursion_reaches_the_same_answer_three_ways() {
    assert_eq!(
        ran("fn.local.ui64 ['fact'] [ui64 'n'] {\n\
                 if [math { 'n' </= ui64 |1| }] { return |1|; }\n\
                 return math { 'n' * fact[math { 'n' - ui64 |1| }] };\n\
             }\n\
             print[fact[|10|] \\n];"),
        "3628800\n"
    );
}

#[test]
fn two_functions_may_call_each_other() {
    // Neither can be written above the other, so this only works because every signature
    // is read before any body is.
    assert_eq!(
        ran("fn.local.bool ['even'] [ui32 'n'] {\n\
                 if [math { 'n' = ui32 |0| }] { return |true|; }\n\
                 return odd[math { 'n' - ui32 |1| }];\n\
             }\n\
             fn.local.bool ['odd'] [ui32 'n'] {\n\
                 if [math { 'n' = ui32 |0| }] { return |false|; }\n\
                 return even[math { 'n' - ui32 |1| }];\n\
             }\n\
             print[even[|7|] \\n];"),
        "false\n"
    );
}

#[test]
fn a_celled_value_survives_being_carried_through_calls() {
    // The case the JIT's cells had to become per-call for: a `b256` held across a call
    // that makes one of its own. Sharing one row of cells would lose the caller's.
    assert_eq!(
        ran("fn.local.b256 ['sum'] [b256 'acc', ui32 'n'] {\n\
                 if [math { 'n' = ui32 |0| }] { return 'acc'; }\n\
                 return sum[math { 'acc' + b256 |0.1| }, math { 'n' - ui32 |1| }];\n\
             }\n\
             print[sum[|0|, |10|] \\n];"),
        "0.999999999999999999999999999999999999999999999999999999999999999999999995472160460586643816245440354556104109658734400930430905208692555032249340703577442630234274019520950273023780351561085172058633563363372331878053955733776092529296875\n"
    );
}

#[test]
fn a_function_that_answers_nothing_still_does_its_work() {
    assert_eq!(
        ran("fn.local.nothing ['greet'] [str 'who'] { print[\"hello, \" 'who' \\n]; }\n\
             greet[|Tankun|];"),
        "hello, Tankun\n"
    );
}

#[test]
fn a_while_loop_stops_when_it_is_told_to() {
    assert_eq!(
        ran("var.local.mut.ui32 ['n'] = [|1|];\n\
             loop.while [math { 'n' < ui32 |100| }] {\n\
                 set ['n'] = [math { 'n' x ui32 |2| }];\n\
             }\n\
             print['n' \\n];"),
        "128\n"
    );
}

#[test]
fn a_while_loop_can_count_its_own_passes() {
    assert_eq!(
        ran("loop.temp.while.ui32 ['pass'] [|true|] {\n\
                 print['pass' \\n];\n\
                 break when reached |3|;\n\
             }"),
        "1\n2\n3\n"
    );
}

#[test]
fn a_perm_counter_afterwards_holds_the_passes_that_ran() {
    // Counted at the start of the pass, so it is 1 during the first and 4 after four --
    // not 5. A counting loop makes the same promise: it never steps past the last value.
    assert_eq!(
        ran("var.local.mut.ui32 ['n'] = [|1|];\n\
             loop.perm.while.ui32 ['passes'] [math { 'n' < ui32 |16| }] {\n\
                 set ['n'] = [math { 'n' x ui32 |2| }];\n\
             }\n\
             print['passes' \\n];"),
        "4\n"
    );
    // And a loop that never runs leaves it at nothing.
    assert_eq!(
        ran("loop.perm.while.ui32 ['passes'] [|false|] { }\nprint['passes' \\n];"),
        "0\n"
    );
}

#[test]
fn break_leaves_the_innermost_loop_and_nothing_more() {
    assert_eq!(
        ran("loop.temp.range.ui8 ['i'] = [|1|, |3|] {\n\
                 loop.temp.range.ui8 ['j'] = [|1|, |9|] {\n\
                     if [math { 'j' > ui8 |2| }] { break; }\n\
                     print['i' 'j' \" \"];\n\
                 }\n\
             }\n\
             print[\\n];"),
        "11 12 21 22 31 32 \n"
    );
}

#[test]
fn an_exact_rational_is_exact_on_all_three_paths() {
    // The sum every article about floating point opens with, and the number no binary
    // float can hold. Both come out right here because nothing rounded.
    assert_eq!(
        ran("var.local.er ['a'] = [|0.1|];\n\
             var.local.er ['b'] = [|0.2|];\n\
             print[math { 'a' + 'b' } \\n];"),
        "3/10\n"
    );
    assert_eq!(
        ran("var.local.er ['t'] = [|1/3|];\n\
             print[math { ('t' + 't') + 't' } \\n];"),
        "1\n"
    );
}

#[test]
fn an_exact_rational_has_no_width_to_overflow() {
    // Two to the sixty-fourth is where a `ui64` gives up. This does not.
    assert_eq!(
        ran("print[math { er |2| ** er |64| } \\n];"),
        "18446744073709551616\n"
    );
    assert_eq!(
        ran("print[math { er |2| ** er |-2| } \\n];"),
        "1/4\n"
    );
}

#[test]
fn an_exact_rational_refuses_what_it_cannot_answer() {
    // The square root of two is not a ratio, so a ratio type will not pretend to have
    // one. Nothing is printed because all three stop before the print -- and `ran` has
    // already insisted they stopped for the same reason as each other.
    assert_eq!(ran("print[math { er |2| ** er |1/2| } \\n];"), "");
    // The same for an answer too large to write down.
    assert_eq!(ran("print[math { er |2| ** er |100000| } \\n];"), "");
    // And for dividing by nothing, which `er` has no infinity to answer with.
    assert_eq!(ran("print[math { er |1| div er |0| } \\n];"), "");
}

#[test]
fn a_decimal_holds_a_tenth_exactly_on_all_three_paths() {
    assert_eq!(
        ran("var.local.d64 ['a'] = [|0.1|];\n\
             var.local.d64 ['b'] = [|0.2|];\n\
             print[math { 'a' + 'b' } \\n];"),
        "0.3\n"
    );
    // The same sum in the binary format of the same width, for contrast.
    assert_eq!(
        ran("var.local.b64 ['a'] = [|0.1|];\n\
             var.local.b64 ['b'] = [|0.2|];\n\
             print[math { 'a' + 'b' } \\n];"),
        "0.3000000000000000444089209850062616169452667236328125\n"
    );
}

#[test]
fn money_keeps_its_cents_on_all_three_paths() {
    assert_eq!(
        ran("var.local.d64 ['price'] = [|19.99|];\n\
             print[math { 'price' x d64 |3| } \" \" math { d64 |20.00| - 'price' } \\n];"),
        "59.97 0.01\n"
    );
}

#[test]
fn a_decimal_is_a_float_and_behaves_like_one() {
    // Unlike `er`, which refuses, a decimal has infinities and NaNs.
    assert_eq!(ran("print[math { d64 |1| div d64 |0| } \\n];"), "inf\n");
    assert_eq!(ran("print[math { d64 |0| div d64 |0| } \\n];"), "nan\n");
    // And each width keeps its own number of digits.
    assert_eq!(ran("print[math { d32 |1| div d32 |3| } \\n];"), "0.3333333\n");
    assert_eq!(
        ran("print[math { d128 |1| div d128 |3| } \\n];"),
        "0.3333333333333333333333333333333333\n"
    );
}

#[test]
fn two_decimals_written_differently_can_be_worth_the_same() {
    // `1.0` and `1.00` are different encodings of one number, so `=` cannot be a
    // comparison of the bits.
    assert_eq!(ran("print[math { d64 |1.0| = d64 |1.00| } \\n];"), "true\n");
}

#[test]
fn an_array_reads_and_writes_the_same_three_ways() {
    assert_eq!(
        ran("var.local.mut.array.5.ui32 ['xs'] = [[|10|, |20|, |30|, |40|, |50|]];\n\
             set ['xs'[|3|]] = [|99|];\n\
             print['xs' \" \" 'xs'[|1|] \" \" count['xs'] \\n];"),
        "[10, 20, 99, 40, 50] 10 5\n"
    );
}

#[test]
fn a_shaped_array_is_flattened_the_same_three_ways() {
    assert_eq!(
        ran("var.local.array.2x3.ui8 ['m'] = [[|1|, |2|, |3|, |4|, |5|, |6|]];\n\
             print['m'[|1|, |1|] \" \" 'm'[|2|, |3|] \" \" 'm'[|1|, |3|] \\n];"),
        "1 6 3\n"
    );
}

#[test]
fn every_element_type_survives_an_array() {
    // The packed widths and the shared kinds take different routes in the JIT, so both
    // are here.
    assert_eq!(
        ran("var.local.array.2.str ['names'] = [[|Tankun|, |Claude|]];\n\
             var.local.array.2.er ['ratios'] = [[|1/3|, |2/7|]];\n\
             var.local.array.2.bool ['flags'] = [[|true|, |false|]];\n\
             var.local.array.2.b256 ['wide'] = [[|0.1|, |0.2|]];\n\
             print['names' \" \" 'ratios' \" \" 'flags' \" \" math { 'wide'[|1|] + 'wide'[|2|] } \\n];"),
        "[Tankun, Claude] [1/3, 2/7] [true, false] 0.30000000000000000000000000000000000000000000000000000000000000000000000181113581576534247350182385817755835613650623962782763791652297798710026371856902294790629039219161989079048785937556593117654657465465106724877841770648956298828125\n"
    );
}

#[test]
fn reaching_past_an_array_stops_the_same_way_three_ways() {
    // Nought is no element, and so is one past the end. `ran` insists all three stopped
    // for the same reason as each other.
    assert_eq!(ran("var.local.array.3.ui8 ['xs'] = [[|1|,|2|,|3|]];\nprint['xs'[|0|]];"), "");
    assert_eq!(ran("var.local.array.3.ui8 ['xs'] = [[|1|,|2|,|3|]];\nprint['xs'[|4|]];"), "");
    assert_eq!(
        ran("var.local.array.2x3.ui8 ['m'] = [[|1|,|2|,|3|,|4|,|5|,|6|]];\nprint['m'[|3|, |1|]];"),
        ""
    );
}

#[test]
fn an_array_walked_by_a_loop_adds_up_the_same_three_ways() {
    assert_eq!(
        ran("var.local.array.5.ui32 ['xs'] = [[|10|, |20|, |30|, |40|, |50|]];\n\
             var.local.mut.ui32 ['total'] = [|0|];\n\
             loop.temp.range.ui32 ['i'] = [|1|, count['xs']] {\n\
                 set ['total'] = [math { 'total' + 'xs'['i'] }];\n\
             }\n\
             print['total' \\n];"),
        "150\n"
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

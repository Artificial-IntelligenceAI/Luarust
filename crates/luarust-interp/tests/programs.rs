//! Whole programs, from source to what they print.
//!
//! This is the end of the pipeline meeting the beginning of it: text goes in, output
//! comes out, and every stage in between has to have agreed.

use luarust_diag::SourceFile;

/// Run a program and return what it printed, insisting it had nothing to complain about.
fn output(source: &str) -> String {
    let file = SourceFile::new("test.lr", source);
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{}", luarust_diag::report(&file, &lexed.errors));
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{}", luarust_diag::report(&file, &parsed.errors));
    let (program, errors) = luarust_check::check(&parsed.program);
    assert!(errors.is_empty(), "{}", luarust_diag::report(&file, &errors));

    let mut out = Vec::new();
    match luarust_interp::run(&program, &mut out) {
        Ok(()) => String::from_utf8(out).expect("output is text"),
        Err(stopped) => panic!("{}", luarust_diag::report(&file, &[stopped.diagnostic()])),
    }
}

/// Run a program expected to stop, and return the fault's code.
fn fault(source: &str) -> &'static str {
    let lexed = luarust_lex::lex(source);
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    let (program, errors) = luarust_check::check(&parsed.program);
    assert!(errors.is_empty(), "expected it to check cleanly: {errors:#?}");
    let mut out = Vec::new();
    luarust_interp::run(&program, &mut out).expect_err("expected this to stop").fault.code
}

#[test]
fn counting_to_five() {
    assert_eq!(
        output("loop.temp.range.ui8 ['i'] = ['1', '5'] { print['i' \\n]; }"),
        "1\n2\n3\n4\n5\n"
    );
}

#[test]
fn a_range_is_inclusive_at_both_ends() {
    assert_eq!(output("loop.temp.range.ui8 ['i'] = ['3', '3'] { print['i']; }"), "3");
    // Counting down is an empty range rather than a reversed one.
    assert_eq!(output("loop.temp.range.ui8 ['i'] = ['5', '1'] { print['i']; }"), "");
}

#[test]
fn a_loop_reaches_the_top_of_its_type_without_going_past_it() {
    // 255 is every bit of a ui8. A counter that stepped before checking would wrap here
    // and never stop.
    assert_eq!(output("loop.temp.range.ui8 ['i'] = ['253', '255'] { print['i' \" \"]; }"), "253 254 255 ");
}

#[test]
fn accumulating_the_readme_way() {
    assert_eq!(
        output(
            "var.local.mut.ui32 ['total'] = ['0'];\n\
             loop.temp.range.ui32 ['i'] = ['1', '10'] { handback 'i' as 'total'; }\n\
             print[\"total is \" 'total' \\n];"
        ),
        "total is 55\n"
    );
}

#[test]
fn a_perm_counter_holds_the_last_value_it_took() {
    // Five, not six. Languages that leak a counter usually leave it one past the end.
    assert_eq!(
        output("loop.perm.range.ui8 ['i'] = ['1', '5'] { } print['i'];"),
        "5"
    );
}

#[test]
fn a_number_prints_as_the_value_that_is_stored() {
    assert_eq!(output("var.local.b16 ['a'] = ['0.1']; print['a'];"), "0.0999755859375");
    assert_eq!(output("var.local.b32 ['a'] = ['0.1']; print['a'];"), "0.10000000149011612");
    assert_eq!(output("var.local.b64 ['a'] = ['0.1']; print['a'];"), "0.1");
    assert_eq!(output("var.local.b16 ['a'] = ['1000']; print['a'];"), "1000");
}

#[test]
fn print_inserts_nothing_at_all() {
    assert_eq!(output("print[\"a\"]; print[\"b\"];"), "ab");
    assert_eq!(output("var.local.ui8 ['x'] = ['7']; print['x' 'x' 'x'];"), "777");
    assert_eq!(output("print[\"a\" \\t \"b\" \\n];"), "a\tb\n");
}

#[test]
fn arithmetic_gives_what_mathematics_gives() {
    let math = |expression: &str| {
        output(&format!("var.local.i32 ['r'] = [math {{ {expression} }}]; print['r'];"))
    };
    assert_eq!(math("2 + 3 * 4"), "14");
    assert_eq!(math("(2 + 3) * 4"), "20");
    assert_eq!(math("2 ** 3 ** 2"), "512");
    assert_eq!(math("-2 ** 2"), "-4");
    assert_eq!(math("100 div 5 div 2"), "10");
    assert_eq!(math("2 x 3 xx 2"), "18");
}

#[test]
fn a_remainder_takes_the_sign_of_the_divisor() {
    let math = |expression: &str| {
        output(&format!("var.local.i32 ['r'] = [math {{ {expression} }}]; print['r'];"))
    };
    // The whole point of choosing mathematics over the C family.
    assert_eq!(math("-7 mod 3"), "2");
    assert_eq!(math("7 mod -3"), "-2");
    assert_eq!(math("7 mod 3"), "1");
    assert_eq!(math("-7 mod -3"), "-1");
}

#[test]
fn a_float_remainder_is_floored_as_well() {
    let math = |expression: &str| {
        output(&format!("var.local.b64 ['r'] = [math {{ {expression} }}]; print['r'];"))
    };
    assert_eq!(math("-7 mod 3"), "2");
    assert_eq!(math("7.5 mod 2"), "1.5");
    assert_eq!(math("-7.5 mod 2"), "0.5");
}

#[test]
fn percent_is_a_fraction_of_a_hundred() {
    assert_eq!(
        output("var.local.b64 ['vat'] = [math { 250 x 20% }]; print['vat'];"),
        "50"
    );
}

#[test]
fn an_integer_wraps_by_default_and_traps_when_asked() {
    assert_eq!(
        output("var.local.ui8 ['x'] = [math { 255 + 1 }]; print['x'];"),
        "0",
        "255 + 1 rolls over to nothing"
    );
    assert_eq!(fault("defaults.overflow.trap; var.local.ui8 ['x'] = [math { 255 + 1 }];"), "R0005");
}

#[test]
fn dividing_a_whole_number_by_zero_stops_and_dividing_a_float_does_not() {
    assert_eq!(fault("var.local.i32 ['x'] = [math { 1 div 0 }];"), "R0002");
    assert_eq!(output("var.local.b64 ['x'] = [math { 1 div 0 }]; print['x'];"), "inf");
    assert_eq!(output("var.local.b64 ['x'] = [math { 0 div 0 }]; print['x'];"), "nan");
}

#[test]
fn the_clock_moves_forward_and_never_back() {
    let out = output(
        "var.local.b64 ['start'] = [time.now];\n\
         loop.temp.range.ui32 ['i'] = ['1', '20000'] { }\n\
         var.local.b64 ['elapsed'] = [math { time.now - 'start' }];\n\
         var.local.b64 ['zero'] = ['0'];\n\
         print['elapsed'];",
    );
    let seconds: f64 = out.parse().expect("a number of seconds");
    assert!(seconds >= 0.0, "monotonic, so never negative: {seconds}");
    assert!(seconds < 60.0, "and twenty thousand empty passes do not take a minute");
}

#[test]
fn the_benchmark_from_the_readme_gives_the_right_answer() {
    // Sum of 1..1,000,000 is 500,000,500,000, and that modulo 1,000,000,007 is 496,500.
    let out = output(
        "var.local.mut.ui64 ['sum'] = ['0'];\n\
         loop.temp.range.ui64 ['i'] = ['1', '1000000'] {\n\
             set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];\n\
         }\n\
         print['sum'];",
    );
    assert_eq!(out, "496500");
}

#[test]
fn a_name_may_be_anything_you_can_type() {
    assert_eq!(
        output("var.local.str ['🧑‍🧑‍🧒‍🧒'] = ['a family']; print['🧑‍🧑‍🧒‍🧒'];"),
        "a family"
    );
    assert_eq!(
        output("var.local.i32 ['a friendly number'] = ['7']; print['a friendly number'];"),
        "7"
    );
}

#[test]
fn the_examples_in_the_repository_all_run() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut ran = 0;
    for entry in std::fs::read_dir(&root).expect("examples/ is there") {
        let path = entry.expect("a readable entry").path();
        if path.extension().is_some_and(|e| e == "lr") {
            let text = std::fs::read_to_string(&path).expect("a readable example");
            // Nothing asserted about what they print -- only that every one of them
            // lexes, parses, checks and runs without complaint.
            output(&text);
            ran += 1;
        }
    }
    assert!(ran >= 4, "expected the examples to be there, found {ran}");
}

// These exercise the compiler, so they are only here when it is. Built as a runtime the
// crate has no front end to feed it a program.
#![cfg(feature = "compile")]

//! The interpreter and the VM, on the same programs, insisting on the same answers.
//!
//! This is the reason the tree-walker is being kept. On its own it proves nothing — it is
//! one implementation and it agrees with itself. Beside a second implementation built a
//! completely different way, every program either produces one answer twice or exposes a
//! bug in one of them.

use luarust_diag::SourceFile;

/// Run a program both ways and insist the two agree, returning what they printed.
fn agreed(source: &str) -> String {
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

    let walked = String::from_utf8(walked).expect("output is text");
    let ran = String::from_utf8(ran).expect("output is text");

    match (walk, vm) {
        (Ok(()), Ok(())) => {
            assert_eq!(
                walked,
                ran,
                "the two paths printed different things\n\n{}",
                chunk.disassemble()
            );
            walked
        }
        (Err(a), Err(b)) => {
            assert_eq!(a.fault.code, b.fault.code, "they stopped for different reasons");
            assert_eq!(walked, ran, "they printed different things before stopping");
            walked
        }
        (Ok(()), Err(b)) => panic!("the VM stopped and the interpreter did not: {:?}", b.fault),
        (Err(a), Ok(())) => panic!("the interpreter stopped and the VM did not: {:?}", a.fault),
    }
}

#[test]
fn counting() {
    assert_eq!(
        agreed("loop.temp.range.ui8 ['i'] = [|1|, |5|] { print['i' \\n]; }"),
        "1\n2\n3\n4\n5\n"
    );
}

#[test]
fn accumulating() {
    assert_eq!(
        agreed(
            "var.local.mut.ui32 ['total'] = [|0|];\n\
             loop.temp.range.ui32 ['i'] = [|1|, |10|] { handback 'i' as 'total'; }\n\
             print[\"total is \" 'total' \\n];"
        ),
        "total is 55\n"
    );
}

#[test]
fn the_awkward_edges_of_a_range() {
    assert_eq!(agreed("loop.temp.range.ui8 ['i'] = [|3|, |3|] { print['i']; }"), "3");
    assert_eq!(agreed("loop.temp.range.ui8 ['i'] = [|5|, |1|] { print['i']; }"), "");
    assert_eq!(
        agreed("loop.temp.range.ui8 ['i'] = [|253|, |255|] { print['i' \" \"]; }"),
        "253 254 255 "
    );
    assert_eq!(agreed("loop.perm.range.ui8 ['i'] = [|1|, |5|] { } print['i'];"), "5");
}

#[test]
fn nested_loops() {
    // Two counters live at once, which is where a register allocator that reuses a
    // register too early would come apart.
    assert_eq!(
        agreed(
            "loop.temp.range.ui8 ['a'] = [|1|, |3|] {\n\
                 loop.temp.range.ui8 ['b'] = [|1|, |3|] { print['a' 'b' \" \"]; }\n\
             }"
        ),
        "11 12 13 21 22 23 31 32 33 "
    );
}

#[test]
fn arithmetic_of_every_shape() {
    let math = |expression: &str| {
        agreed(&format!("var.local.i32 ['r'] = [math {{ {expression} }}]; print['r'];"))
    };
    assert_eq!(math("2 + 3 * 4"), "14");
    assert_eq!(math("(2 + 3) * 4"), "20");
    assert_eq!(math("2 ** 3 ** 2"), "512");
    assert_eq!(math("-2 ** 2"), "-4");
    assert_eq!(math("-7 mod 3"), "2");
    assert_eq!(math("7 mod -3"), "-2");
    assert_eq!(math("100 div 5 div 2"), "10");
}

#[test]
fn every_float_width_agrees() {
    for ty in ["b16", "b32", "b64", "b128", "b256"] {
        let source = format!(
            "var.local.{ty} ['a'] = [|0.1|];\n\
             var.local.{ty} ['b'] = [math {{ 'a' + 'a' }}];\n\
             var.local.{ty} ['c'] = [math {{ 'b' x 'b' div 'a' }}];\n\
             print['a' \" \" 'b' \" \" 'c'];"
        );
        let out = agreed(&source);
        assert!(!out.is_empty(), "{ty} printed nothing");
    }
}

#[test]
fn a_value_written_into_itself() {
    // `'sum'` is both a source and the destination, so a compiler that wrote the answer
    // before reading both sides would give the wrong number.
    assert_eq!(
        agreed(
            "var.local.mut.i32 ['sum'] = [|10|];\n\
             set ['sum'] = [math { 'sum' x 'sum' + 'sum' }];\n\
             print['sum'];"
        ),
        "110"
    );
}

#[test]
fn faults_happen_in_the_same_place_both_ways() {
    // Both stop, for the same reason, having printed the same thing first.
    agreed("print[\"before\"]; var.local.i32 ['x'] = [math { 1 div 0 }];");
    agreed("defaults.overflow.trap; var.local.ui8 ['x'] = [math { 255 + 1 }];");
}

#[test]
fn the_benchmark_agrees() {
    assert_eq!(
        agreed(
            "var.local.mut.ui64 ['sum'] = [|0|];\n\
             loop.temp.range.ui64 ['i'] = [|1|, |100000|] {\n\
                 set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];\n\
             }\n\
             print['sum'];"
        ),
        // The sum of 1 to 100,000 is 5,000,050,000, and that modulo 1,000,000,007 is
        // 49,965 -- which is what taking the remainder every pass leaves behind.
        "49965"
    );
}

/// Run a program both ways and insist neither stops, without comparing what they printed.
///
/// For programs that read the clock. `time.now` is deliberately not the same twice — the
/// two paths run at different speeds, so they *should* report different elapsed times, and
/// comparing them would be asserting that the VM is exactly as slow as the interpreter.
fn both_ran(source: &str) {
    let file = SourceFile::new("test.lr", source);
    let lexed = luarust_lex::lex(source);
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    let (program, errors) = luarust_check::check(&parsed.program);
    assert!(errors.is_empty(), "{}", luarust_diag::report(&file, &errors));

    let mut walked = Vec::new();
    luarust_interp::run(&program, &mut walked).expect("the interpreter ran it");
    let mut ran = Vec::new();
    luarust_vm::run(&luarust_vm::compile(&program), &mut ran).expect("the VM ran it");
}

#[test]
fn every_example_in_the_repository_agrees() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut checked = 0;
    for entry in std::fs::read_dir(&root).expect("examples/ is there") {
        let path = entry.expect("a readable entry").path();
        if path.extension().is_some_and(|e| e == "lr") {
            let text = std::fs::read_to_string(&path).expect("a readable example");
            if text.contains("time.now") {
                both_ran(&text);
            } else {
                agreed(&text);
            }
            checked += 1;
        }
    }
    assert!(checked >= 4, "expected the examples to be there, found {checked}");
}

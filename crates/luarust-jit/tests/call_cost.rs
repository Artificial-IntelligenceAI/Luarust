//! What one call into kept code costs, before its body runs.
//!
//! A measurement rather than a test, kept so the number in
//! `notes/the-hot-jit-enters-mid-loop.md` can be re-derived instead of believed. Its
//! interpreted counterpart is two programs on the plain VM at N=3,000,000 — the leaf
//! called per iteration against the same body written inline:
//!
//!     fn.local.i64 ['leaf'] [i64 'n'] { return math { ('n' * 3) mod 1000000007 }; }
//!     set ['sum'] = [math { ('sum' + leaf['i']) mod 1000000007 }];       // 171 ms
//!     set ['sum'] = [math { ('sum' + (('i' * 3) mod 1000000007)) mod 1000000007 }];  // 56 ms
//!
//! (171 − 56) ms over 3,000,000 calls = 38 ns for the whole interpreted call. The
//! measurement below prices the kept-code entry for the same leaf. On an M5 it said
//! 151 ns — four interpreted calls spent before the body runs — which is what makes a
//! call-counting policy with no body filter a slowdown, and a slim re-entry the
//! prerequisite for one with.

use luarust_diag::SourceFile;

#[test]
#[ignore = "a measurement, run by hand when the entry path changes"]
fn cost_of_one_kept_call() {
    let source = "fn.local.i64 ['leaf'] [i64 'n'] {\n\
                  return math { ('n' * 3) mod 1000000007 };\n\
                  }\n\
                  var.local.i64 ['a'] = [leaf[|5|]];\n\
                  print['a' \\n];\n";
    let file = SourceFile::new("test.lr", source);
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{}", luarust_diag::report(&file, &lexed.errors));
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{}", luarust_diag::report(&file, &parsed.errors));
    let (program, errors) = luarust_check::check(&parsed.program);
    assert!(errors.is_empty(), "{}", luarust_diag::report(&file, &errors));
    let chunk = luarust_vm::compile(&program);

    let code = luarust_jit::compile_routine(&chunk, 0).expect("a leaf compiles");
    let started = std::time::Instant::now();
    let slots = chunk.funcs[0].registers;
    // The same shape the VM hands over: the top level's frame lent, the fresh one the
    // call built handed over, argument in slot nought.
    let top = vec![luarust_core::value::Value::Bool(false); chunk.registers];
    let open = [&top];
    let fresh = |n: u64| {
        let mut fresh = vec![luarust_core::value::Value::Bool(false); slots];
        fresh[0] = luarust_core::value::Value::Num { ty: luarust_core::Ty::I64, bits: n };
        fresh
    };

    // Warm up, and insist on the right answer while at it.
    let mut out = Vec::new();
    let answer = code.call(&open, fresh(5), started, &mut out).expect("it runs");
    assert_eq!(
        answer,
        Some(luarust_core::value::Value::Num { ty: luarust_core::Ty::I64, bits: 15 })
    );

    const CALLS: u64 = 200_000;
    let t0 = std::time::Instant::now();
    let mut checksum = 0u64;
    for n in 1..=CALLS {
        let mut out = Vec::new();
        let answer = code.call(&open, fresh(n), started, &mut out).expect("it runs");
        if let Some(luarust_core::value::Value::Num { bits, .. }) = answer {
            checksum = checksum.wrapping_add(bits);
        }
    }
    let spent = t0.elapsed();
    assert_eq!(checksum, 60_000_300_000, "the calls stopped answering correctly");
    println!(
        "{CALLS} kept calls in {spent:?} = {} ns/call",
        spent.as_nanos() / u128::from(CALLS)
    );
}

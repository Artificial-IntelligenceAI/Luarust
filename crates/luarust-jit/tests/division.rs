//! What `div` and `mod` mean, checked against arithmetic rather than against each other.
//!
//! The three paths agreeing proves they are the same implementation, not that the
//! implementation is right — and for a long time they agreed on a `div` that truncated
//! beside a `mod` that floored, so `(a div b) x b + (a mod b)` was not `a`. Every case
//! here is compared to a quotient and a remainder worked out in Rust, and the identity
//! is asserted on top, for all three conventions and both signs of both operands.

use luarust_check::Start;
use luarust_core::value::Division;
use luarust_diag::SourceFile;

/// Run one program three ways under one convention, insisting on a single answer.
fn three_ways(source: &str, division: Division) -> String {
    let file = SourceFile::new("test.lr", source);
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{}", luarust_diag::report(&file, &lexed.errors));
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{}", luarust_diag::report(&file, &parsed.errors));
    let (program, errors) =
        luarust_check::check_with(&parsed.program, Start { division, ..Start::default() });
    assert!(errors.is_empty(), "{}", luarust_diag::report(&file, &errors));

    let mut walked = Vec::new();
    luarust_interp::run(&program, &mut walked).expect("the interpreter finished");
    let chunk = luarust_vm::compile(&program);
    let mut ran = Vec::new();
    luarust_vm::run(&chunk, &mut ran).expect("the VM finished");
    assert_eq!(
        String::from_utf8_lossy(&walked),
        String::from_utf8_lossy(&ran),
        "the interpreter and the VM disagree\n\n{source}"
    );
    let mut compiled = Vec::new();
    match luarust_jit::run(&chunk, &mut compiled) {
        Err(declined) => panic!("the JIT declined this: {}\n\n{source}", declined.because),
        Ok(outcome) => {
            outcome.expect("the compiled program finished");
            assert_eq!(
                String::from_utf8_lossy(&walked),
                String::from_utf8_lossy(&compiled),
                "the compiled program printed something else\n\n{source}"
            );
        }
    }
    String::from_utf8_lossy(&walked).into_owned()
}

/// The same three conventions, worked out here from first principles rather than by
/// calling the code under test.
fn expected(division: Division, a: i64, b: i64) -> (i64, i64) {
    // Start from real division and round the quotient the way the convention says; the
    // remainder is then whatever is left, which is the definition, not a correction.
    let exact = a as f64 / b as f64;
    let quotient = match division {
        Division::Floored => exact.floor(),
        Division::Truncated => exact.trunc(),
        Division::Euclidean if b > 0 => exact.floor(),
        Division::Euclidean => exact.ceil(),
    } as i64;
    (quotient, a - quotient * b)
}

fn conventions() -> [(Division, &'static str); 3] {
    [
        (Division::Floored, "floored"),
        (Division::Truncated, "truncated"),
        (Division::Euclidean, "euclidean"),
    ]
}

/// Every sign of every operand, in a type wide enough that the reference arithmetic in
/// `f64` is exact.
const NUMBERS: [i64; 10] = [-7, -6, -3, -2, -1, 1, 2, 3, 6, 7];

#[test]
fn a_grid_of_signs() {
    for (division, name) in conventions() {
        for a in NUMBERS {
            let mut source = String::new();
            let mut want = String::new();
            for b in NUMBERS {
                source.push_str(&format!(
                    "print[math {{ i32 |{a}| div i32 |{b}| }} \" \" \
                       math {{ i32 |{a}| mod i32 |{b}| }} \\n];\n"
                ));
                let (q, r) = expected(division, a, b);
                want.push_str(&format!("{q} {r}\n"));
                // The identity that started this: a quotient and a remainder describe one
                // division, and putting them back together has to give the dividend.
                assert_eq!(q * b + r, a, "{name}: {a} and {b} do not put back together");
            }
            assert_eq!(three_ways(&source, division), want, "{name}, dividend {a}");
        }
    }
}

#[test]
fn what_each_convention_promises() {
    for (division, name) in conventions() {
        for a in NUMBERS {
            for b in NUMBERS {
                let (_, r) = expected(division, a, b);
                match division {
                    // The remainder follows the divisor.
                    Division::Floored => assert!(r == 0 || (r < 0) == (b < 0), "{name}"),
                    // The remainder follows the dividend.
                    Division::Truncated => assert!(r == 0 || (r < 0) == (a < 0), "{name}"),
                    // The remainder is never negative, and never reaches the divisor.
                    Division::Euclidean => assert!(r >= 0 && r < b.abs(), "{name}"),
                }
            }
        }
    }
}

/// Values the checker cannot see through, so the compiled code takes the general path
/// rather than the one the range analysis shortcuts.
#[test]
fn operands_the_checker_cannot_see() {
    for (division, name) in conventions() {
        // `i` runs 1 to 7; negating it and dividing gives every sign without a constant
        // for the checker to reason from.
        let source = "loop.temp.range.i32 ['i'] = [|1|, |7|] {\n\
                          var.local.i32 ['a'] = [math { |0| - 'i' }];\n\
                          print[math { 'a' div |3| } \" \" math { 'a' mod |3| } \" \" \
                                math { 'a' div |-3| } \" \" math { 'a' mod |-3| } \\n];\n\
                      }";
        let mut want = String::new();
        for i in 1..=7i64 {
            let (q1, r1) = expected(division, -i, 3);
            let (q2, r2) = expected(division, -i, -3);
            want.push_str(&format!("{q1} {r1} {q2} {r2}\n"));
        }
        assert_eq!(three_ways(source, division), want, "{name}");
    }
}

/// The one case that overflows is the same case in all three conventions, because there
/// the division is exact and there is nothing to round.
#[test]
fn the_most_negative_value() {
    for (division, name) in conventions() {
        let source = "var.local.i32 ['a'] = [|-2147483648|];\n\
                      print[math { 'a' div |-1| } \" \" math { 'a' mod |-1| }];";
        assert_eq!(three_ways(source, division), "-2147483648 0", "{name}");
    }
}

/// Unsigned types cannot tell the conventions apart, and must not be made slower or
/// different by one being chosen.
#[test]
fn unsigned_is_unmoved() {
    for (division, name) in conventions() {
        let source = "print[math { ui32 |7| div ui32 |3| } \" \" math { ui32 |7| mod ui32 |3| }];";
        assert_eq!(three_ways(source, division), "2 1", "{name}");
    }
}

/// What the range analysis believes about a remainder.
///
/// It knew that a remainder is smaller than its divisor and concluded it could not be
/// negative — true of a floored one, and true of a euclidean one, and false of a
/// truncated one, which follows the dividend. A later `div` then looked like it had a
/// nonnegative operand, the JIT emitted the unsigned instruction on the strength of it,
/// and `-1 div 2` came out as two billion while the other two paths said nought.
#[test]
fn a_truncated_remainder_can_be_negative_and_the_analysis_knows() {
    let source = "var.local.mut.i32 ['a'] = [|-7|];\n\
                  var.local.i32 ['r'] = [math { 'a' mod i32 |3| }];\n\
                  print['r' \" \" math { 'r' div i32 |2| } \" \" math { 'r' mod i32 |2| }];";
    assert_eq!(three_ways(source, Division::Floored), "2 1 0");
    assert_eq!(three_ways(source, Division::Truncated), "-1 0 -1");
    assert_eq!(three_ways(source, Division::Euclidean), "2 1 0");
}

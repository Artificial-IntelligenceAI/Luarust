//! The same program four ways, insisting on one answer.
//!
//! The tiering engine is the fourth, and the only one that runs a program *both* ways:
//! interpreted until a loop proves itself, compiled from there on, with the registers
//! carried across the join. Everything the other three can get wrong it can get wrong
//! too, and one thing they cannot -- handing a running program over.
//!
//! The threshold is turned down to a handful here. At ten thousand no generated program
//! would ever reach it, and the point is to make the switch happen, in as many different
//! places in as many different programs as possible. Turning it down also puts the join
//! somewhere awkward on purpose: partway through a loop, with registers holding whatever
//! the first few passes left in them.

use luarust_core::value::{Engine, Value};
use luarust_diag::SourceFile;
use luarust_vm::{Chunk, Taken, Tier};

/// A tier that compiles the moment a loop goes round `after` times.
struct Eagerly {
    after: u32,
    /// Whether anything was actually handed over, so a test can tell a program that
    /// switched from one that never had a loop to switch in.
    switched: bool,
    /// Whether any of it was a loop inside a routine, which is the harder handover: it
    /// comes back.
    inside: bool,
}

impl Tier for Eagerly {
    fn threshold(&self) -> u32 {
        self.after
    }

    fn hot(
        &mut self,
        chunk: &Chunk,
        routine: Option<usize>,
        at: usize,
        frames: Vec<Vec<Value>>,
        started: std::time::Instant,
        out: &mut dyn std::io::Write,
    ) -> Taken {
        self.switched = true;
        if routine.is_some() {
            self.inside = true;
        }
        let taken = match routine {
            None => luarust_jit::resume(chunk, at, frames, started, out).map(Taken::Finished),
            Some(index) => luarust_jit::resume_routine(chunk, index, at, frames, started, out)
                .map(Taken::Returned),
        };
        taken.unwrap_or(Taken::Declined)
    }
}

struct Ran {
    text: String,
    fault: Option<&'static str>,
}

fn ended(out: Vec<u8>, outcome: Result<(), luarust_check::value::Stopped>) -> Ran {
    Ran {
        text: String::from_utf8_lossy(&out).into_owned(),
        fault: outcome.err().map(|stopped| stopped.fault.code),
    }
}

/// Run one program every way there is, and insist they all say the same thing.
///
/// Returns whether the tiering engine actually switched, so a caller can tell that its
/// programs are exercising the thing and not merely passing through it.
fn four_ways(source: &str, after: u32) -> bool {
    four_ways_reporting(source, after).0
}

/// The same, saying both whether anything switched and whether any of it was inside a
/// routine.
fn four_ways_reporting(source: &str, after: u32) -> (bool, bool) {
    let file = SourceFile::new("test.lr", source);
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{}", luarust_diag::report(&file, &lexed.errors));
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{}", luarust_diag::report(&file, &parsed.errors));
    let (program, errors) = luarust_check::check(&parsed.program);
    assert!(errors.is_empty(), "{}", luarust_diag::report(&file, &errors));

    let mut out = Vec::new();
    let outcome = luarust_interp::run(&program, &mut out);
    let walked = ended(out, outcome);

    let mut chunk = luarust_vm::compile(&program);
    chunk.engine = Engine::Hot;
    let mut out = Vec::new();
    let outcome = luarust_vm::run(&chunk, &mut out);
    let interpreted = ended(out, outcome);

    let mut out = Vec::new();
    let outcome = luarust_jit::run(&chunk, &mut out).expect("the JIT takes every chunk");
    let compiled = ended(out, outcome);

    let mut tier = Eagerly { after, switched: false, inside: false };
    let mut out = Vec::new();
    let outcome = luarust_vm::run_with(&chunk, &mut out, Some(&mut tier));
    let tiered = ended(out, outcome);

    for (name, ran) in
        [("the VM", &interpreted), ("the whole-program JIT", &compiled), ("the tiering engine", &tiered)]
    {
        assert_eq!(ran.text, walked.text, "{name} printed something else\n\n{source}");
        assert_eq!(ran.fault, walked.fault, "{name} ended differently\n\n{source}");
    }
    (tier.switched, tier.inside)
}

#[test]
fn a_loop_handed_over_part_way_through() {
    // The registers at the join hold a partial sum and a counter that is not where it
    // started; getting either of them wrong changes the answer.
    let source = "var.local.mut.ui32 ['sum'] = [|0|];\n\
                  loop.temp.range.ui32 ['i'] = [|1|, |100|] {\n\
                      set ['sum'] = [math { 'sum' + 'i' }];\n\
                  }\n\
                  print['sum'];";
    for after in 1..=8 {
        assert!(four_ways(source, after), "nothing switched at {after}");
    }
}

#[test]
fn everything_the_registers_can_hold_survives_the_join() {
    // One of each family, all live across the join: a narrow float, a wide one, a
    // decimal, a rational, a string and a bool. Anything the handover drops or garbles
    // shows up in what is printed after the loop.
    let source = "var.local.b64 ['f'] = [|0.1|];\n\
                  var.local.b256 ['w'] = [|0.1|];\n\
                  var.local.d64 ['d'] = [|19.99|];\n\
                  var.local.er ['e'] = [|1/3|];\n\
                  var.local.str ['s'] = [|hello|];\n\
                  var.local.bool ['b'] = [|true|];\n\
                  var.local.mut.i32 ['n'] = [|0|];\n\
                  loop.temp.range.i32 ['i'] = [|1|, |20|] {\n\
                      set ['n'] = [math { 'n' - 'i' }];\n\
                  }\n\
                  print['f' \" \" 'w' \" \" 'd' \" \" 'e' \" \" 's' \" \" 'b' \" \" 'n'];";
    for after in 1..=5 {
        assert!(four_ways(source, after), "nothing switched at {after}");
    }
}

#[test]
fn an_array_made_before_the_join_is_still_there_after_it() {
    // The heap is the VM's when the switch happens, and compiled code must inherit it
    // rather than start a new one -- and must not sweep the arrays the VM is still using.
    let source = "var.local.array.5.ui32 ['xs'] = [[|10|, |20|, |30|, |40|, |50|]];\n\
                  var.local.mut.ui32 ['sum'] = [|0|];\n\
                  loop.temp.range.ui32 ['i'] = [|1|, count['xs']] {\n\
                      set ['sum'] = [math { 'sum' + 'xs'['i'] }];\n\
                  }\n\
                  print['sum'];";
    // A loop of five goes round four times, so four is the last threshold it can reach.
    for after in 1..=4 {
        assert!(four_ways(source, after), "nothing switched at {after}");
    }
}

#[test]
fn a_fault_after_the_join_is_the_same_fault() {
    let source = "var.local.mut.i32 ['n'] = [|10|];\n\
                  loop.temp.range.i32 ['i'] = [|1|, |20|] {\n\
                      set ['n'] = [math { 'n' div ('i' - i32 |15|) }];\n\
                  }\n\
                  print['n'];";
    for after in 1..=4 {
        four_ways(source, after);
    }
}

#[test]
fn nested_loops_switch_at_whichever_goes_round_first() {
    let source = "var.local.mut.ui32 ['n'] = [|0|];\n\
                  loop.temp.range.ui32 ['a'] = [|1|, |5|] {\n\
                      loop.temp.range.ui32 ['b'] = [|1|, |5|] {\n\
                          set ['n'] = [math { 'n' + ('a' x 'b') }];\n\
                      }\n\
                  }\n\
                  print['n'];";
    for after in 1..=9 {
        assert!(four_ways(source, after), "nothing switched at {after}");
    }
}

#[test]
fn a_loop_that_calls_something_still_calls_it_after_the_join() {
    let source = "fn.local.ui32 ['double'] [ui32 'x'] { return math { 'x' x ui32 |2| }; }\n\
                  var.local.mut.ui32 ['n'] = [|0|];\n\
                  loop.temp.range.ui32 ['i'] = [|1|, |10|] {\n\
                      set ['n'] = [math { 'n' + double['i'] }];\n\
                  }\n\
                  print['n'];";
    for after in 1..=5 {
        assert!(four_ways(source, after), "nothing switched at {after}");
    }
}

/// Generated programs, all four ways, switching wherever they happen to have a loop.
#[test]
fn generated_programs_agree_four_ways() {
    let mut switched = 0;
    for seed in 1..=400 {
        if four_ways(&luarust_gen::program(seed).source, 1 + seed as u32 % 4) {
            switched += 1;
        }
    }
    // Not every generated program has a loop in it, but plenty must, or this test is
    // running the tiering engine without ever tiering.
    assert!(switched > 100, "only {switched} of 400 switched, which is too few to prove much");
}

/// The deep version of the above, for when something changes about the handover.
///
/// Not part of the ordinary gate, because every program here compiles a whole module
/// through LLVM twice over.
#[test]
#[ignore = "a deep sweep for changes to the handover, not for every gate"]
fn twenty_thousand_agree_four_ways() {
    let (mut switched, mut inside) = (0, 0);
    for seed in 1..=20_000 {
        let (took, in_routine) =
            four_ways_reporting(&luarust_gen::program(seed).source, 1 + seed as u32 % 5);
        switched += usize::from(took);
        inside += usize::from(in_routine);
    }
    println!("{switched} of 20000 handed over part way through, {inside} inside a routine");
    assert!(switched > 5_000);
    // The harder handover is the one that comes back, so a sweep that never took a loop
    // inside a routine would be leaving half of this untested.
    assert!(inside > 100, "only {inside} were inside a routine");
}

/// A loop inside a routine, which is the handover that has to come back.
///
/// The top level runs to the end of the program and never returns, so getting that wrong
/// shows up as a wrong answer at most. A routine has to give an answer back, pop its
/// frame, and leave the call underneath exactly as the VM left it -- and then the VM has
/// to carry on interpreting a caller that has been sitting there the whole time.
#[test]
fn a_loop_inside_a_routine_comes_back() {
    let source = "fn.local.ui64 ['triangle'] [ui64 'n'] {\n\
                      var.local.mut.ui64 ['sum'] = [|0|];\n\
                      loop.temp.range.ui64 ['i'] = [|1|, 'n'] {\n\
                          set ['sum'] = [math { 'sum' + 'i' }];\n\
                      }\n\
                      return 'sum';\n\
                  }\n\
                  print[triangle[|10|] \" \" triangle[|100|] \" \" triangle[|4|]];";
    for after in 1..=6 {
        let (switched, inside) = four_ways_reporting(source, after);
        assert!(switched, "nothing switched at {after}");
        assert!(inside, "the switch was not inside the routine at {after}");
    }
}

/// The same, for a routine whose answer cannot live in a machine register.
#[test]
fn a_routine_that_answers_with_something_celled() {
    let source = "fn.local.er ['sixths'] [ui32 'n'] {\n\
                      var.local.mut.er ['sum'] = [|0|];\n\
                      loop.temp.range.ui32 ['i'] = [|1|, 'n'] {\n\
                          set ['sum'] = [math { 'sum' + er |1/6| }];\n\
                      }\n\
                      return 'sum';\n\
                  }\n\
                  print[sixths[|6|] \" \" sixths[|3|]];";
    for after in 1..=4 {
        let (_, inside) = four_ways_reporting(source, after);
        assert!(inside, "the switch was not inside the routine at {after}");
    }
}

/// A routine that gives nothing back, and one that is called from inside another.
#[test]
fn a_routine_with_no_answer_and_one_nested_in_another() {
    let source = "fn.local.nothing ['count'] [ui32 'n'] {\n\
                      loop.temp.range.ui32 ['i'] = [|1|, 'n'] { print['i']; }\n\
                  }\n\
                  fn.local.nothing ['twice'] [ui32 'n'] {\n\
                      loop.temp.range.ui32 ['j'] = [|1|, |2|] { count['n']; }\n\
                  }\n\
                  twice[|4|];\n\
                  print[\" done\"];";
    for after in 1..=5 {
        let (_, inside) = four_ways_reporting(source, after);
        assert!(inside, "the switch was not inside a routine at {after}");
    }
}

/// A routine that faults after the join, which must fault the same way and in the same
/// place as it would have on the VM.
#[test]
fn a_routine_that_stops_after_the_join() {
    let source = "fn.local.i32 ['grind'] [i32 'n'] {\n\
                      var.local.mut.i32 ['r'] = [|100|];\n\
                      loop.temp.range.i32 ['i'] = [|1|, 'n'] {\n\
                          set ['r'] = [math { 'r' div ('i' - i32 |9|) }];\n\
                      }\n\
                      return 'r';\n\
                  }\n\
                  print[grind[|20|]];";
    for after in 1..=5 {
        four_ways_reporting(source, after);
    }
}

// These exercise the compiler, so they are only here when it is. Built as a runtime the
// crate has no front end to feed it a program.
#![cfg(feature = "compile")]

//! Writing a chunk out and reading it back.
//!
//! Two things are being checked. That a program written on one machine is the same program
//! when read on another — which is the whole of "compile once, run anywhere". And that a
//! file which is *not* a valid chunk produces a complaint rather than a crash, because
//! "run anywhere" means chunks will arrive from places nobody vouched for.

use luarust_core::value::Division;
use luarust_vm::serialize::{self, Broken, Source};

fn compiled(source: &str) -> luarust_vm::Chunk {
    compiled_with(source, Division::default())
}

fn compiled_with(source: &str, division: Division) -> luarust_vm::Chunk {
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{:#?}", lexed.errors);
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{:#?}", parsed.errors);
    let (program, errors) = luarust_check::check_with(
        &parsed.program,
        luarust_check::Start { division, ..luarust_check::Start::default() },
    );
    assert!(errors.is_empty(), "{errors:#?}");
    luarust_vm::compile(&program)
}

fn output_of(chunk: &luarust_vm::Chunk) -> String {
    let mut out = Vec::new();
    luarust_vm::run(chunk, &mut out).expect("it ran");
    String::from_utf8(out).expect("output is text")
}

const COUNTING: &str = "loop.temp.range.ui8 ['i'] = [|1|, |5|] { print['i' \\n]; }";

const EVERYTHING: &str = "\
var.local.mut.i64 ['sum'] = [|0|];\n\
var.local.b64 ['ratio'] = [math { 1 div 3 }];\n\
var.local.b256 ['wide'] = [|0.1|];\n\
var.local.str ['who'] = [|🧑‍🧑‍🧒‍🧒|];\n\
loop.perm.range.i64 ['i'] = [|1|, |20|] {\n\
    set ['sum'] = [math { ('sum' + 'i') mod 7 }];\n\
}\n\
print['who' \" \" 'sum' \" \" 'ratio' \" \" 'wide' \" \" 'i' \\n];";

#[test]
fn a_chunk_written_out_reads_back_the_same() {
    for source in [COUNTING, EVERYTHING] {
        let chunk = compiled(source);
        let bytes = serialize::write(&chunk, "test.lr", source);
        let loaded = serialize::read(&bytes).expect("it read back");

        assert_eq!(loaded.path, "test.lr");
        let Source::Text(travelled) = &loaded.source else {
            panic!("the source travels with the chunk");
        };
        assert_eq!(travelled, source);
        assert_eq!(loaded.chunk.code, chunk.code);
        assert_eq!(loaded.chunk.consts, chunk.consts);
        assert_eq!(loaded.chunk.texts, chunk.texts);
        assert_eq!(loaded.chunk.spans, chunk.spans);
        assert_eq!(loaded.chunk.registers, chunk.registers);
        assert_eq!(loaded.chunk.overflow, chunk.overflow);
        assert_eq!(output_of(&loaded.chunk), output_of(&chunk), "and it runs the same");
    }
}

#[test]
fn the_wide_types_and_the_awkward_names_survive_the_trip() {
    let chunk = compiled(EVERYTHING);
    let loaded = serialize::read(&serialize::write(&chunk, "test.lr", EVERYTHING)).expect("read");
    let out = output_of(&loaded.chunk);
    assert!(out.starts_with("🧑‍🧑‍🧒‍🧒 "), "{out}");
    assert!(out.contains("0.3333333333333333"), "{out}");
}

#[test]
fn overflow_policy_travels_too() {
    let chunk = compiled("defaults.overflow.trap; var.local.ui8 ['x'] = [|1|]; print['x'];");
    let loaded = serialize::read(&serialize::write(&chunk, "t.lr", "")).expect("read");
    assert_eq!(loaded.chunk.overflow, luarust_check::value::Overflow::Trap);
}

#[test]
fn it_is_little_endian_whatever_the_machine_is() {
    // The promise of the format. The version is the first number after the magic, and it
    // has to be written low byte first no matter who wrote it.
    let bytes = serialize::write(&compiled(COUNTING), "t.lr", "");
    assert_eq!(&bytes[..8], serialize::MAGIC);
    assert_eq!(&bytes[8..12], &serialize::VERSION.to_le_bytes());
}

#[test]
fn what_is_not_a_chunk_is_refused() {
    assert_eq!(serialize::read(b"").unwrap_err(), Broken::Truncated);
    assert_eq!(serialize::read(b"not a chunk at all").unwrap_err(), Broken::NotAChunk);

    let mut wrong_version = serialize::write(&compiled(COUNTING), "t.lr", "");
    wrong_version[8..12].copy_from_slice(&99u32.to_le_bytes());
    assert_eq!(serialize::read(&wrong_version).unwrap_err(), Broken::Version(99));

    // Cut short anywhere and it says so rather than reading past the end.
    let whole = serialize::write(&compiled(COUNTING), "t.lr", "");
    for cut in 0..whole.len() {
        let outcome = serialize::read(&whole[..cut]);
        assert!(outcome.is_err(), "a chunk cut to {cut} bytes was accepted");
    }
}

#[test]
fn a_chunk_that_points_at_nothing_is_refused() {
    // Say the program has no registers, which makes every register it names out of range.
    // Built rather than patched: poking a byte offset means the test has an opinion about
    // where a field sits, and it goes quietly wrong the day a field is added before it.
    let mut chunk = compiled(COUNTING);
    chunk.registers = 0;
    let broken = serialize::write(&chunk, "t.lr", "");
    match serialize::read(&broken) {
        Err(Broken::OutOfRange { what, .. }) => assert_eq!(what, "register"),
        other => panic!("expected a register complaint, got {other:?}"),
    }
}

#[test]
fn no_byte_anywhere_can_make_it_panic() {
    // The property that matters most. A chunk arriving from somewhere else may be
    // anything at all, and the only two acceptable outcomes are a chunk or a complaint.
    //
    // This used to stop at `read`, and that is how it missed the thing it is named for: a
    // type tag is a byte like any other, a flipped bit turns a valid tag into a *different*
    // valid tag, the chunk loads without complaint, and the VM panics when the register
    // turns out to hold something else. Loading it is half the test. Running it is the
    // other half.
    //
    // Every single-bit flip, because that is what makes a tag wrong rather than invalid --
    // `^ 0xff` on a tag gives a number no type has, which `read` refuses, and the
    // interesting case never runs.
    let source = "var.local.mut.ui8 ['n'] = [|3|];\n\
                  var.local.bool ['yes'] = [|true|];\n\
                  if [math { 'yes' }] { set ['n'] = [math { 'n' + ui8 |1| }]; }\n\
                  print['n' \\n];\n";
    let whole = serialize::write(&compiled(source), "test.lr", source);

    let mut checked = 0;
    for at in 0..whole.len() {
        for bit in 0..8u32 {
            let mut broken = whole.clone();
            broken[at] ^= 1 << bit;
            checked += 1;

            // A corrupt constant can describe a program that never finishes -- a loop
            // whose step became nought is still a program, and no reading of it could say
            // otherwise. So this runs in a thread and lets a slow one be, while a panic
            // still comes back as one.
            //
            // The bytes cross the thread, not the chunk: a chunk holds `Rc`s and is not
            // `Send`, and reading is half of what is being tested anyway.
            let (tell, hear) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let outcome = std::panic::catch_unwind(|| {
                    if let Ok(loaded) = serialize::read(&broken) {
                        let mut sink = Vec::new();
                        let _ = luarust_vm::run(&loaded.chunk, &mut sink);
                    }
                });
                let _ = tell.send(outcome.is_ok());
            });
            match hear.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(true) => {}
                Ok(false) => panic!("a bit flip at byte {at}, bit {bit} panicked the VM"),
                // Still running: a program that does not finish, which is allowed.
                Err(_) => {}
            }
        }
    }
    assert!(checked > 1000, "only tried {checked} corruptions");
}

#[test]
fn every_generated_program_survives_the_trip() {
    for seed in 1..=2000 {
        let source = luarust_gen::program(seed).source;
        let lexed = luarust_lex::lex(&source);
        let parsed = luarust_parse::parse(&source, &lexed.tokens);
        let (program, errors) = luarust_check::check(&parsed.program);
        assert!(errors.is_empty(), "seed {seed} did not check");

        let chunk = luarust_vm::compile(&program);
        let loaded = serialize::read(&serialize::write(&chunk, "g.lr", &source))
            .unwrap_or_else(|why| panic!("seed {seed} would not read back: {why}"));
        assert_eq!(loaded.chunk.code, chunk.code, "seed {seed}");

        // And running the one that came off disk does what running the original does.
        let mut before = Vec::new();
        let a = luarust_vm::run(&chunk, &mut before);
        let mut after = Vec::new();
        let b = luarust_vm::run(&loaded.chunk, &mut after);
        assert_eq!(before, after, "seed {seed} printed differently after a round trip");
        assert_eq!(
            a.err().map(|s| s.fault.code),
            b.err().map(|s| s.fault.code),
            "seed {seed} ended differently after a round trip"
        );
    }
}

#[test]
fn a_chunk_can_be_written_without_its_source_and_still_know_the_line() {
    let chunk = compiled(EVERYTHING);
    let with = serialize::write_with(&chunk, "test.lr", EVERYTHING, true, false);
    let without = serialize::write_with(&chunk, "test.lr", EVERYTHING, false, false);
    assert!(without.len() < with.len(), "dropping the source should make it smaller");

    let loaded = serialize::read(&without).expect("it read back");
    let Source::Lines { starts, len } = &loaded.source else {
        panic!("only the line table should have travelled");
    };
    assert_eq!(starts.len(), EVERYTHING.lines().count());
    assert_eq!(*len, EVERYTHING.len());
    // The text is gone and the program is not: it still runs, and it runs the same.
    assert_eq!(output_of(&loaded.chunk), output_of(&chunk));

    // And every span in it still lands on the line it always did.
    let full = luarust_diag::SourceFile::new("test.lr", EVERYTHING);
    let thin = loaded.source.file("test.lr");
    for span in &loaded.chunk.spans {
        assert_eq!(
            thin.position(span.start).line,
            full.position(span.start).line,
            "line of {span:?}"
        );
    }
    assert!(!thin.has_text());
}

#[test]
fn a_line_table_that_could_not_have_come_from_a_file_is_refused() {
    let chunk = compiled(COUNTING);
    let mut bytes = serialize::write_with(&chunk, "t.lr", COUNTING, false, false);

    // Where the line table begins: the magic, then a word each for the version, overflow,
    // collecting, float printing, the engine and the division, then the register count,
    // then the path, then the flag saying there is no source and the count of lines.
    let table = 8 + (4 * 7) + (4 + "t.lr".len()) + 4 + 4;

    // Checked rather than trusted. This is a byte offset into a format that grows a field
    // now and then, and a test that pokes the wrong four bytes still passes for the wrong
    // reason -- which is exactly what happened when `[run] mode` was added.
    assert_eq!(
        u32::from_le_bytes(bytes[table..table + 4].try_into().expect("four bytes")),
        0,
        "the first line begins at nought, so this is not the line table any more"
    );

    // The first line has to begin at nought. Say it began somewhere else.
    bytes[table..table + 4].copy_from_slice(&7u32.to_le_bytes());
    assert!(serialize::read(&bytes).is_err(), "a bogus line table must be refused");
}

#[test]
fn a_decimal_survives_being_written_either_way() {
    // The two encodings hold the same numbers, so a chunk written in one and read back
    // gives the same program -- which is the whole claim that lets it be a setting.
    let source = "var.local.d64 ['x'] = [|19.99|];\nprint[math { 'x' x d64 |3| } \\n];";
    let chunk = compiled(source);
    let bid = serialize::read(&serialize::write_with(&chunk, "t.lr", source, true, false))
        .expect("bid reads");
    let dpd = serialize::read(&serialize::write_with(&chunk, "t.lr", source, true, true))
        .expect("dpd reads");
    assert_eq!(output_of(&bid.chunk), "59.97\n");
    assert_eq!(output_of(&dpd.chunk), output_of(&bid.chunk));
    // And the files really are different, or the setting would be doing nothing.
    assert_ne!(
        serialize::write_with(&chunk, "t.lr", source, true, false),
        serialize::write_with(&chunk, "t.lr", source, true, true)
    );
}

/// A chunk that says it wants more registers than an instruction could name.
///
/// Every other index in a chunk is checked against something. This was the count itself,
/// checked against nothing, so a chunk claiming four billion registers was accepted and
/// the VM then tried to build four billion values — about ninety-six gigabytes, for a
/// program of nine registers. One flipped byte in a file was enough to ask for it.
#[test]
fn a_chunk_cannot_ask_for_registers_nothing_could_name() {
    // Built rather than patched, so the test has no opinion about where the field sits.
    let asking = |registers: usize| {
        let mut chunk = compiled("var.local.ui64 ['n'] = [|1|]; print['n'];");
        chunk.registers = registers;
        serialize::read(&serialize::write(&chunk, "test.lr", "")).map(|_| ())
    };

    // What an instruction can name is fine, however wasteful.
    assert!(asking(65_536).is_ok(), "a `Reg` reaches 65,536 of them");

    // One more than that cannot be named by anything, so it is refused rather than made.
    for asked in [65_537, 100_000_000, 4_000_000_000] {
        assert!(
            matches!(asking(asked), Err(Broken::TooManyRegisters { .. })),
            "{asked} registers should be refused, not allocated"
        );
    }
}

/// The convention travels with the chunk.
///
/// A chunk is the artefact that runs somewhere else, and `div` means different things
/// under different settings — so the setting has to be *in* the file. If it were read
/// from the project file at run time instead, the same chunk would give different answers
/// depending on which folder it was run from, which is the opposite of what a chunk is.
#[test]
fn a_chunk_carries_the_division_it_was_compiled_under() {
    let source = "print[math { i32 |-7| div i32 |3| } \" \" math { i32 |-7| mod i32 |3| }];";
    let answers = [
        (Division::Floored, "-3 2"),
        (Division::Truncated, "-2 -1"),
        (Division::Euclidean, "-3 2"),
    ];
    for (division, want) in answers {
        let chunk = compiled_with(source, division);
        let read = serialize::read(&serialize::write_with(&chunk, "t.lr", source, false, false))
            .expect("it reads back");
        assert_eq!(read.chunk.division, division, "the setting survived the round trip");
        assert_eq!(output_of(&read.chunk), want, "{division:?}");
    }
}

/// A chunk whose division tag is not one of the three.
#[test]
fn a_division_nothing_could_have_written_is_refused() {
    let source = COUNTING;
    let chunk = compiled(source);
    let mut bytes = serialize::write_with(&chunk, "t.lr", source, false, false);
    // The magic, then a word each for the version, overflow, collecting and float
    // printing, then the engine, then the division.
    let division = 8 + (4 * 5);
    assert_eq!(
        u32::from_le_bytes(bytes[division..division + 4].try_into().expect("four bytes")),
        Division::Floored.tag(),
        "the default was compiled in, so this is not the division any more"
    );
    bytes[division..division + 4].copy_from_slice(&9u32.to_le_bytes());
    assert!(serialize::read(&bytes).is_err(), "an unknown division must be refused");
}

/// A damaged chunk is refused for being damaged, not for whatever it happens to say.
///
/// Before this, every flipped bit had to be caught by whichever field it landed in — and
/// most were, because every index is range-checked and every tag is looked up. But a bit
/// that lands in a *value* changes what a program computes, and there was nothing to
/// notice. A chunk that has been damaged is not a program anybody wrote, and saying so is
/// better than running something plausible.
#[test]
fn a_damaged_chunk_is_refused_for_being_damaged() {
    let source = COUNTING;
    let chunk = compiled(source);
    let good = serialize::write_with(&chunk, "t.lr", source, false, false);
    assert!(serialize::read(&good).is_ok(), "the chunk it wrote must read back");

    let mut caught = 0;
    let mut missed = 0;
    for byte in 0..good.len() {
        for bit in 0..8u32 {
            let mut damaged = good.clone();
            damaged[byte] ^= 1 << bit;
            match serialize::read(&damaged) {
                Err(serialize::Broken::Damaged) => caught += 1,
                // The magic and the version come before the sum and answer for themselves.
                Err(_) => missed += 1,
                Ok(_) => panic!("a chunk with byte {byte} bit {bit} flipped read as whole"),
            }
        }
    }
    assert!(caught > 0, "the sum caught nothing");
    // Only the twelve bytes of magic and version are allowed to be caught by something
    // else, since they are read before the sum is.
    assert!(missed <= 12 * 8, "{missed} flips were not caught by the sum");
}

/// The sum costs nothing that changes an answer.
#[test]
fn summing_does_not_change_what_a_chunk_says() {
    let source = EVERYTHING;
    let chunk = compiled(source);
    let bytes = serialize::write_with(&chunk, "t.lr", source, true, false);
    let read = serialize::read(&bytes).expect("it reads back");
    assert_eq!(output_of(&read.chunk), output_of(&chunk), "the round trip changed the program");
}

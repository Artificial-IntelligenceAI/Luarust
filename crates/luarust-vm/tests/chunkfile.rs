// These exercise the compiler, so they are only here when it is. Built as a runtime the
// crate has no front end to feed it a program.
#![cfg(feature = "compile")]

//! Writing a chunk out and reading it back.
//!
//! Two things are being checked. That a program written on one machine is the same program
//! when read on another — which is the whole of "compile once, run anywhere". And that a
//! file which is *not* a valid chunk produces a complaint rather than a crash, because
//! "run anywhere" means chunks will arrive from places nobody vouched for.

use luarust_vm::serialize::{self, Broken, Source};

fn compiled(source: &str) -> luarust_vm::Chunk {
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{:#?}", lexed.errors);
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{:#?}", parsed.errors);
    let (program, errors) = luarust_check::check(&parsed.program);
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
    let chunk = compiled(COUNTING);
    let whole = serialize::write(&chunk, "t.lr", "");

    // Find the first instruction and turn its destination register into one that is not
    // there. The VM would index straight past the end of its registers.
    let mut broken = whole.clone();
    let code_at = broken
        .windows(8)
        .position(|w| w == b"\x00\x00\x00\x00\x00\x00\x00\x00")
        .unwrap_or(0);
    let _ = code_at;

    // Simpler and surer: rewrite the register count to zero, which makes every register
    // in the program out of range.
    broken[16..20].copy_from_slice(&0u32.to_le_bytes());
    match serialize::read(&broken) {
        Err(Broken::OutOfRange { what, .. }) => assert_eq!(what, "register"),
        other => panic!("expected a register complaint, got {other:?}"),
    }
}

#[test]
fn no_byte_anywhere_can_make_it_panic() {
    // The property that matters most. A chunk arriving from somewhere else may be
    // anything at all, and the only two acceptable outcomes are a chunk or a complaint.
    let whole = serialize::write(&compiled(EVERYTHING), "test.lr", EVERYTHING);
    let mut checked = 0;
    for at in 0..whole.len() {
        for flip in [0x01u8, 0x80, 0xff] {
            let mut broken = whole.clone();
            broken[at] ^= flip;
            // Either it reads, or it says why. Panicking is the failure.
            let _ = serialize::read(&broken);
            checked += 1;
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
    // The first line has to begin at nought. Say it began somewhere else.
    let table = 8 + 4 + 4 + 4 + (4 + "t.lr".len()) + 4 + 4;
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

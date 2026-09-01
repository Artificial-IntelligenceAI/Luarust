//! Every path that produces machine code calls `optimise`.
//!
//! Forgetting it does not fail: the code is correct, the tests pass, the fuzzer agrees on
//! all 200,000 programs, and the result is 43% slower for no visible reason. That is not
//! hypothetical -- it is how the JIT shipped until somebody read the emitted IR and asked
//! why a module built at `OptimizationLevel::Aggressive` was full of allocas and branches
//! against a literal divisor. That flag is the *codegen* level and runs no IR passes.
//!
//! A comment cannot fail, and a timing assertion is flaky and wants a quiet machine. What
//! cannot be faked is the shape of the IR: an unoptimised module gives every register an
//! `alloca` and reloads it, and `mem2reg` is the first thing any pipeline does. So each
//! path is asked for its IR and told to have no register allocas left in it.

use luarust_diag::SourceFile;

const PROGRAM: &str = "var.local.mut.ui64 ['sum'] = [|0|];\n\
                       loop.temp.range.ui64 ['i'] = [|1|, |1000|] {\n\
                           set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];\n\
                       }\n\
                       print['sum'];";

fn chunk_of(source: &str) -> luarust_vm::Chunk {
    let file = SourceFile::new("test.lr", source);
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "{}", luarust_diag::report(&file, &lexed.errors));
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "{}", luarust_diag::report(&file, &parsed.errors));
    let (program, errors) = luarust_check::check(&parsed.program);
    assert!(errors.is_empty(), "{}", luarust_diag::report(&file, &errors));
    luarust_vm::compile(&program)
}

/// A register's stack slot, which `mem2reg` removes and nothing else makes.
fn register_allocas(ir: &str) -> Vec<&str> {
    ir.lines()
        .map(str::trim)
        .filter(|line| line.contains("= alloca") && line.starts_with("%r"))
        .collect()
}

#[test]
fn both_paths_to_machine_code_are_optimised() {
    let chunk = chunk_of(PROGRAM);

    let in_memory = luarust_jit::emit_ir(&chunk).expect("the JIT takes this");
    let ahead_of_time = luarust_jit::emit_native_ir(&chunk).expect("the native path takes this");

    for (name, ir) in [("the in-memory JIT", &in_memory), ("the native path", &ahead_of_time)] {
        let left = register_allocas(ir);
        assert!(
            left.is_empty(),
            "{name} did not optimise: {} register allocas survived, starting {:?}.\n\
             Something on this path is not calling `optimise`. See \
             notes/every-path-calls-optimise.md.",
            left.len(),
            left.first()
        );
    }
}

/// The two paths differ in what surrounds the program, not in the program.
///
/// The native module carries a `main` and a table of bytes that the in-memory one has no
/// need for. Everything else is the same emitter over the same chunk, so a difference
/// anywhere else is a difference in what the two of them *do*.
#[test]
fn the_native_path_adds_a_main_and_its_tables_and_nothing_else() {
    let chunk = chunk_of(PROGRAM);
    let in_memory = luarust_jit::emit_ir(&chunk).expect("the JIT takes this");
    let ahead_of_time = luarust_jit::emit_native_ir(&chunk).expect("the native path takes this");

    assert!(ahead_of_time.contains("@luarust_tables"), "the tables must travel with it");
    assert!(ahead_of_time.contains("define i32 @main("), "it needs an entry point");
    assert!(!in_memory.contains("@luarust_tables"), "in memory the tables are handed over");
    assert!(!in_memory.contains("define i32 @main("), "in memory Rust is the entry point");

    // The helpers each path declares must be the same set. In memory they are bound by
    // address; in a file the linker binds them by name from `luarust-native`, and a name
    // only one path knows about is a link error waiting for whoever ships first.
    let declared = |ir: &str| {
        let mut names: Vec<String> = ir
            .lines()
            .filter(|line| line.starts_with("declare "))
            .filter_map(|line| line.split('@').nth(1))
            .map(|rest| rest.split('(').next().unwrap_or("").to_string())
            .filter(|name| name.starts_with("luarust_"))
            .collect();
        names.sort();
        names.dedup();
        names
    };
    let mut theirs = declared(&ahead_of_time);
    // `luarust_start` and `luarust_finish` are the two the native path alone needs.
    theirs.retain(|name| name != "luarust_start" && name != "luarust_finish");
    assert_eq!(declared(&in_memory), theirs, "the two paths want different helpers");
}

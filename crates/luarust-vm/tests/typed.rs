//! The load-time typing pass, held against the compiler and against liars.
#![cfg(feature = "compile")]

use luarust_core::Ty;
use luarust_vm::chunk::Op;

fn chunk_of(source: &str) -> luarust_vm::Chunk {
    let lexed = luarust_lex::lex(source);
    assert!(lexed.ok(), "lexing failed: {:#?}", lexed.errors);
    let parsed = luarust_parse::parse(source, &lexed.tokens);
    assert!(parsed.ok(), "parsing failed: {:#?}", parsed.errors);
    let (program, errors) = luarust_check::check(&parsed.program);
    assert!(errors.is_empty(), "checking failed: {errors:#?}");
    luarust_vm::compile(&program)
}

/// Everything the compiler produces types. The pass exists to refuse strangers, and a
/// single false refusal of our own compiler's output is a bug in the pass.
#[test]
fn five_thousand_compiled_chunks_all_type() {
    for seed in 1..=5_000 {
        let chunk = luarust_vm::compile(&{
            let program = luarust_gen::program(seed);
            let lexed = luarust_lex::lex(&program.source);
            let parsed = luarust_parse::parse(&program.source, &lexed.tokens);
            let (checked, errors) = luarust_check::check(&parsed.program);
            assert!(errors.is_empty(), "seed {seed} did not check");
            checked
        });
        if let Err(broken) = luarust_vm::typed::well_typed(&chunk) {
            panic!("seed {seed} was refused: {broken}");
        }
    }
}

/// A chunk that lies about a register is refused, wherever the lie sits.
#[test]
fn a_lying_instruction_is_refused() {
    let mut chunk = chunk_of(
        "var.local.mut.i64 ['x'] = [|5|];\n\
         set ['x'] = [math { 'x' + 1 }];\n\
         print['x' \\n];\n",
    );
    assert!(luarust_vm::typed::well_typed(&chunk).is_ok(), "the honest chunk types");

    // Find the add and make it claim its operands are a type nothing wrote.
    let lied = chunk
        .code
        .iter()
        .position(|op| matches!(op, Op::Binary { .. }))
        .expect("the program adds");
    let Op::Binary { op, dst, lhs, rhs, nonnegative, .. } = chunk.code[lied] else {
        unreachable!("just found it");
    };
    chunk.code[lied] = Op::Binary { op, ty: Ty::U8, dst, lhs, rhs, nonnegative };
    assert!(
        luarust_vm::typed::well_typed(&chunk).is_err(),
        "an instruction lying about its operands was believed"
    );
}

/// Reading a register no path has written is refused; so is reading one that two paths
/// wrote as different types. Writing the disagreement is fine — only reading it lies.
#[test]
fn the_unwritten_and_the_disagreed_are_refused_only_when_read() {
    let mut chunk = chunk_of(
        "var.local.mut.i64 ['x'] = [|5|];\n\
         print['x' \\n];\n",
    );
    // Point the print at a register nothing wrote.
    let read = chunk
        .code
        .iter()
        .position(|op| matches!(op, Op::PrintValue { .. }))
        .expect("the program prints");
    let Op::PrintValue { ty, .. } = chunk.code[read] else { unreachable!() };
    // A register the frame holds and nothing writes: one past everything the compiler
    // used.
    chunk.registers += 1;
    let unwritten = (chunk.registers - 1) as u16;
    chunk.code[read] = Op::PrintValue { src: unwritten, ty };
    assert!(
        luarust_vm::typed::well_typed(&chunk).is_err(),
        "reading a register nothing wrote was believed"
    );

    // Two arms writing one register as different types is the compiler's own habit —
    // dead afterwards, and honest as long as nothing reads it after the join.
    let disagreeing = chunk_of(
        "var.local.i64 ['c'] = [|1|];\n\
         var.local.mut.i64 ['x'] = [|0|];\n\
         if [math { 'c' = 1 }] { var.local.i64 ['t'] = [|7|]; set ['x'] = ['t']; }\n\
         else { var.local.b64 ['u'] = [|1.5|]; print['u' \\n]; }\n\
         print['x' \\n];\n",
    );
    assert!(
        luarust_vm::typed::well_typed(&disagreeing).is_ok(),
        "a dead disagreement was refused"
    );
}

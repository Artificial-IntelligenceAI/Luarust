# Task 3, mid-flight: where it stands and what was decided

One line: the value-range analysis is designed and approved but not yet written; the
ir carries the flag (always false so far), and every decision that needed Tankun is
already made — the rest is implementation.

## Decisions made (do not re-open)

- **Where the analysis lives**: `luarust-check`, at AST/ir level, as the brief said —
  a range loop declares its counter's bounds outright, where chunk code would need
  loops pattern-matched back out of jumps.
- **How the fact travels**: a flag on the chunk's `Binary` op, set by `compile` from
  the checker's analysis. Chunk VERSION goes 12 -> 13. An older `.lrc` lacks the flag
  and keeps the correction — safe degradation. Approved by Tankun via luarust-a3:
  "flag in the chunk", option 1.
- **Who consumes it**: the JIT only (`int_division` in `luarust-jit/src/lib.rs:1789`
  drops the zero-guard, the minus-one select and the floored correction when the flag
  says dividend >= 0 and divisor > 0). Both interpreters ignore it and keep computing
  floored mod the long way, so the three-way fuzz agreement stays the net that
  catches an over-eager proof.
- **Known and accepted**: the flag is the one field `serialize.rs`'s `check()` cannot
  validate — a proof indexes nothing. A hand-flipped flag makes the JIT silently
  compute a truncated remainder where floored differs. Tankun accepted this
  knowingly; say so in the code and the commit when the serialize change lands.

## Done so far (this commit)

- `ir::Expr::Binary` carries `nonnegative: bool`, documented, `false` at all three
  construction sites; both consumers match with `..`. Compiles, tests green,
  clippy clean in all three configs, fuzz 200000 all agreed.

## Not done

1. The analysis itself — new module in `luarust-check` (sketch: interval
   [i128, i128] per slot; `Const` exact; `Load` slot-or-ty-range; add/sub/mul in
   i128 then the overflow rule: computed within the ty stays, else Trap intersects
   and Wrap widens to the ty's full range; floored `mod` by a divisor proven [1, d]
   lands in [0, d-1]; range `Loop` seeds the counter [from.lo, to.hi] and iterates
   the body to a fixpoint, a few rounds then widen; `While` kills every slot its
   body stores; `If` joins arm exits by hull; calls answer their ty's range;
   commit pass sets the flag only from the stable state). The benchmark's proof
   works under the default overflow=wrap because the intervals never leave i64.
2. `Op::Binary` gains the flag; sites are known: chunk.rs:162, compile.rs:268/339
   (false) and :552 (from ir), serialize.rs:284/929 + VERSION 13, vm lib.rs matches
   (add `..` — the VM never reads it), jit lib.rs:809 passes it to `int_division`.
3. `int_division` consumes it (signed Div and Mod only).
4. Measure: brief says the correction is worth 273 -> 242 ms on the M5 benchmark;
   `luarust jit bench.lr` before and after, and the u64-declared loop should no
   longer beat the i64 one.
5. Gates as always: tests, three clippys, fuzz 200000, plus `-p luarust-jit` tests
   (three_ways is what catches a wrong proof reaching the JIT).

## State of the wider branch

Tasks 1 and 2 are done and verified — see [x86-interpreter-gap.md](x86-interpreter-gap.md)
and the JIT reachability commit (324797e). The diagnose workflow is deleted; its
eight rounds live in the branch history.

## For whoever starts iteration 3 (not task 3)

**AOT must call `optimise` too.** It will be a third path that produces machine code, and
the two that exist both call it. Forgetting it is silent: correct output, green tests, the
fuzzer agreeing, and 43% slower than it should be. The doc comment on `optimise` in
`luarust-jit/src/lib.rs` says so, because that is how the JIT shipped until somebody read
the IR it was emitting.

A comment does not fail, so write the guard that does — and do not write it as a timing
assertion, which is flaky and needs a machine to be quiet. Both paths run the same emitter
over the same chunk, so **the optimised IR they produce must be identical**:

```rust
// The ahead-of-time path and the in-memory one differ in where the machine code goes,
// not in what it says. If they ever disagree, one of them is missing a pass.
assert_eq!(emit_ir(&chunk), emit_ir_for_native(&chunk));
```

That is deterministic, runs in milliseconds, needs no benchmark, and fails loudly the day
somebody adds a fourth path and forgets. It also catches the opposite mistake — a pass
added to one path and not the other — which no benchmark would ever tell you about.

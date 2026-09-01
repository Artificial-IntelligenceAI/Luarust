# Every path that produces machine code calls `optimise`

There are two today, `run` and `emit_ir`, and ahead-of-time output will be a third.
Forgetting the call does not fail: the code is correct, the tests pass, the fuzzer agrees
on all 200,000 programs, and the result is 43% slower for no visible reason.

Which is not hypothetical. It is how the JIT shipped until somebody read the emitted IR and
asked why a module optimised at `OptimizationLevel::Aggressive` was full of allocas and
branches against a literal divisor. That flag is the *codegen* level and runs no IR passes;
nothing had ever asked LLVM to optimise anything. Turning it on was 1.85× C to 1.05× — the
largest single win this language has had, from one function call.

## Write the guard, not the comment

A comment cannot fail, and a timing assertion is flaky and needs a quiet machine. Both
paths run the same emitter over the same chunk, so the optimised IR they produce must be
identical:

```rust
// The ahead-of-time path and the in-memory one differ in where the machine code goes,
// not in what it says. If they ever disagree, one of them is missing a pass.
assert_eq!(emit_ir(&chunk), emit_ir_for_native(&chunk));
```

Deterministic, runs in milliseconds, needs no benchmark, and fails the day somebody adds a
fourth path and forgets. It also catches the mistake in the other direction — a pass added
to one path and not the other — which no benchmark would ever report.

That shape is why everything else here works: the three execution paths agreeing, the
editor's guards comparing themselves against the compiler. Do not assert that an answer is
good; assert that two things which must match still match.

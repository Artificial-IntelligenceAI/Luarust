I'm handing you a performance investigation on Luarust, a language I'm building. I make
the design decisions; you're here for the measurements and the optimisation. Nothing in
this task requires changing what the language means.

## The job, hardest first

Luarust has three execution paths — a tree-walking interpreter, a bytecode VM, and an LLVM
JIT — and they must agree bit for bit. Relative to C on the same dependent-chain benchmark,
on the same commit:

                        Apple M5     x86-64 (GitHub runner)
    Luarust JIT            1.05x        1.34x
    Luarust VM             4.23x        9.37x
    Luarust tree-walker    9.79x       25.84x
    CPython               14.43x       18.45x

**The two Luarust interpreters lose 2.2-2.6x relative to C when moving to x86-64, and
nothing else in the table does.** CPython moves 1.28x — and CPython is also a branch-heavy
bytecode interpreter, so this is not simply "x86-64 is worse at interpreting". The
tree-walker changes places with CPython between the machines. Nobody has explained it.

That is the first task: find out why, and fix what can be fixed. Profile on x86-64, not on
the Mac — the effect barely exists there.

Two smaller ones, both diagnosed already:

1. **The JIT emits a floored-`mod` correction it cannot prove is dead.** Luarust's `mod` is
   floored, so the IR carries `icmp slt` + `select` around every `srem`. On the benchmark
   both operands are provably non-negative (`sum` is only ever assigned `x mod 1000000007`;
   the counter starts at 1), but LLVM only knows the static type. Declaring the same loop
   `ui64` drops the correction and goes 273 ms -> 242. A value-range analysis in
   `luarust-check` would close it properly.
2. **The JIT compiles every routine whether it runs or not**, about 0.2 ms each, so 1,000
   unused functions cost 215 ms of startup. Step one is skipping unreachable routines:
   every `Call` names its target index and the language has no function pointers, so the
   reachable set from `main` is exact. Roughly twenty lines.

## What has already been tried, and rejected

Don't rediscover these. All measured.

- **An `Rc<RefCell<..>>` variant in `Value` cost the VM 15%** in *drop glue* alone, on
  programs containing no arrays. Arrays are `u32` handles inside `Value::Num` for this
  reason. Do not put a reference-counted thing in `Value`.
- **Inlining an array's shape into `Ty` made it 10 bytes and cost ~6%** on both interpreted
  paths. `Ty` is 2 bytes and rides in every value and instruction. Keep it small.
- Four profiler wins are already banked and documented in the README's "How fast it is":
  boxing `Fault` (VM 18%, tree-walker 11%), an integer-comparison fast path (4%/9%), the
  VM's two-loop structure (4%), inlining `int_op` (2%).

## How correctness is established

This is why the work is safe to do aggressively. `luarust fuzz N` generates N programs and
checks that all three paths agree bit for bit, faults included. Before you report anything
as done:

    cargo test --workspace
    cargo clippy --workspace --all-targets
    cargo clippy -p luarust-cli --features jit --all-targets
    cargo clippy -p luarust-vm --no-default-features --all-targets
    cargo run --release -p luarust-cli -- fuzz 200000

The JIT needs `LLVM_SYS_211_PREFIX` pointing at an LLVM 21 install. Use 200,000 for the
fuzz, not 20,000 — the smaller count misses things.

`.github/workflows/benchmark.yml` runs the whole comparison table on x86-64 in one sitting.
It is `workflow_dispatch` only. That is how the numbers above were produced.

## There is another Claude on this machine

The session that built most of this is running alongside you, named **`luarust-2b`**. It is
Claude Opus 5 and it knows why things are the way they are: what was measured, what was
tried and abandoned, and which decisions are mine and settled.

Use it. `ListAgents` will show it once it is up, and `SendMessage` reaches it. Ask it
before assuming something is an accident — several things in this codebase look like
oversights and are not (`Ty` being two bytes, arrays being handles rather than a `Value`
variant, the JIT compiling every routine). It can also tell you what I have already
refused, which will save you proposing it.

It has no authority over this work. If it says something you can measure, measure it.

## Constraints

- **Work on a branch, not `main`.** I push to `main` from the web while you work, and so
  does `luarust-2b`.
- **Don't change what the language means.** No syntax, no semantics, no new types. If an
  optimisation needs a language change, tell me and stop.
- **Don't touch iteration 3's architecture**: ahead-of-time native output, a hot JIT that
  compiles only what runs, and inline shell blocks. Those are designed and mine.
- Don't add features, refactor, or introduce abstractions beyond what the task requires. A
  bug fix doesn't need surrounding cleanup. Don't design for hypothetical future
  requirements. This codebase has a deliberate style; match what is around you.
- Before reporting progress, audit each claim against a tool result from this session. Only
  report work you can point to evidence for; if something is not yet verified, say so. If
  tests fail, say so with the output. If a step was skipped, say that.
- When you have enough information to act, act. Give me a recommendation, not a survey.

## Keep notes

Write one lesson per file under `notes/`, with a one-line summary at the top. Record what
worked and what didn't, and why it mattered. Don't record what the repo or git history
already says. Update an existing note rather than adding a duplicate; delete notes that
turn out to be wrong.

Start with the x86-64 interpreter question. Scope it, tell me what you find, and go.

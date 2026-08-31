# The x86-64 interpreter gap is struct traffic, not the divider

One line: the interpreters lose on x86-64 because they move `Value` and `Result`
structs through the stack on the dependent chain — Apple silicon makes store-to-load
forwarding nearly free and Zen 4 charges every hop — not because of `mod`'s division.

## What was ruled out, with the measurement that ruled it out

- **Divide latency.** The obvious suspect: the benchmark's hot op is `mod` by a
  runtime divisor (a real `idiv`), C and the JIT strength-reduce the constant, and
  x86 dividers are slow on paper. Killed by a C control on the runner itself: C with
  the modulus from argv (one `idiv` per iteration, verified present by objdump) ran
  *no slower* than C with the constant (389 vs 424 ms at N=1e8). The runner's EPYC
  9V74 (Zen 4) early-terminates division on small quotients just as the M5 does, and
  this benchmark's quotient is 0..2. On the M5 the same pair differs by 0.3 ns/iter.
- **Anything mod-specific at all.** A control loop with the same op count but `-`
  instead of `mod`: VM 3634 ms vs VM mod 3626; interp 12070 vs 12242. The mod op
  costs what a subtract costs. What looked like a mod delta in round 1 was just the
  extra instruction.

## What the profiles showed (perf on the runner, task-clock sampling)

- vm-add: 40% of runtime on one instruction, `movupd %xmm0,(%r14)` — the 16-byte
  store writing a `Value` into the registers `Vec`.
- interp-add: the top five instructions are all `movaps %xmm0, N(%rsp)` — `Value` /
  `Result` structs stored into stack frames, narrow loads of the same bytes right
  after. This is `eval()` returning structs by value on every AST node.
- Also visible: `int_op` does not get inlined on x86 despite `#[inline]`, and its
  own match dispatch (`lea` + `jmp *%rax`) is a large share of what remains.

The asymmetry: M5-class cores rename memory and forward store-to-load at ~zero
cycles; Zen 4 pays ~7-8 cycles per hop, and every hop is on the dependent chain.
CPython barely moves between the machines because it shuffles 8-byte pointers, which
forward cheaply, and its per-iteration overhead drowns the difference anyway.

## The numbers (N=1e8, ns/iter, best of 3, same sitting per machine)

                     M5      x86 (EPYC 9V74)   ratio
  C const mod        2.47      4.24            1.7x   <- the machine itself
  VM add             9.2      24.9             2.7x
  interp add        18.2      92.1             5.1x   <- no divide anywhere

Ordered by how much struct traffic each does: C none, VM some, tree-walker most.

## First fix banked

Boxing the `Fault` inside `Stopped` (commit 14c87bc) — see
[stopped-was-the-second-instance.md](stopped-was-the-second-instance.md).

## Measurement mechanics worth keeping

- The GitHub runner has no PMU (`cycles` etc. are `<not supported>`), but
  `perf record -e task-clock` samples fine, and release builds carry debuginfo, so
  per-instruction annotation works. Ship annotates out as artifacts; the log
  truncates them.
- A workflow that exists only on a branch cannot be `workflow_dispatch`ed (gh
  resolves the file against the default branch). Trigger on `push` to the branch
  instead; push events run the file as of the pushed commit.
- The runner's numbers move by a third between sittings. Every comparison must be
  interleaved inside one job. Same rule locally: the M5 drifts with thermals, so
  A/B by alternating binaries, not by before/after.

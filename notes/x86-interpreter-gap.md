# The x86-64 interpreter gap is a vector store read back narrow

One line: rustc writes every `Value` into the register file (and every struct return
onto the stack) as a 16-byte vector store, the interpreters read the fields back with
narrow scalar loads, and x86-64 cannot store-forward that shape — the load stalls
until the store drains to cache, ~12x the matched-width cost on Zen 3, while Apple
silicon forwards it for free.

## The proof

A C probe (round 7, `.github/workflows/diagnose.yml` history) of the same dependent
add-chain two ways, N=1e8:

                          Zen 3 (EPYC 7763)      Apple M5
  scalar stores, matched loads     156 ms          187 ms
  vector store, narrow loads      1837 ms          225 ms     <- 12x vs 1.2x

Getting the wide store emitted took three attempts: gcc splits struct copies,
16-byte `memcpy`, and even `_mm_storeu_si128` into scalar moves at -O2. Only a
`volatile __m128i` write survives. Check `objdump | grep -c movups` before trusting
any such probe.

This is the shape in every x86 profile of the interpreters: `movupd %xmm0,(%r14)`
carrying 40-48% of the VM's runtime (the `Value` store into the registers `Vec`),
`movaps %xmm0, N(%rsp)` as the tree-walker's entire top five (struct returns from
`eval()`), with scalar loads of `bits` and the discriminant right behind. It explains
the original table completely: C never does it, the VM does it once per op, the
tree-walker several times per node — and CPython is exempt because its pointer
traffic is width-matched 8-byte stores and loads.

## What was ruled out on the way, each by measurement

- **Divide latency**: C with a real per-iteration `idiv` matches strength-reduced C
  on the runner; modern server dividers early-terminate on small quotients.
- **Anything mod-specific**: a same-op-count subtract loop costs exactly what the
  mod loop costs, both interpreters.
- **`Stopped` being 96 bytes**: boxing it was flat on x86 (kept for the M5 win).
- **Width-matched store-forwarding**: three matched hops per step cost ~1.4 ns/iter
  on both machines. The mismatch is the crime, not the hop.
- **The `int_op` call**: `inline(always)` buys ~5% on x86.
- **Dispatch count**: the micro-op VM (single dispatch) wins ~5% on M5, loses ~5% on
  Zen 3. Dispatch was never the x86 story.

## The fix, and its verdict

Match the widths. When an integer result's destination register already holds a
`Num`, write `ty` and `bits` in place as scalar stores instead of assigning the
whole enum (five lines in the VM's `int_arm`). Interleaved, same sitting:

                    baseline    fixed
  VM mod, Zen 4      3663 ms    2330 ms   -36%   (9.7x C -> 6.2x C)
  VM add, Zen 4      2251 ms    1696 ms   -25%
  VM mod, M5         1051 ms     749 ms   -29%

The tree-walker's version of the disease is its by-value `Result<Value, Stopped>`
returns. Tankun's call: leave it. The interpreters are not the performance story —
the JIT is 1.05x C and exists precisely so nobody tortures an interpreter; the
tree-walker's job is to be the oracle, and its speed only prices fuzz throughput.

## Measurement mechanics worth keeping

- The runner pool is heterogeneous: EPYC 7763 (Zen 3), EPYC 9V74 (Zen 4), Xeon
  8370C (Ice Lake), Xeon 6973P (Granite Rapids) all appeared within one day, with
  ~40% drift between sittings. Nothing compares across runs; interleave every
  comparison inside one job, and carry a C control for the machine itself.
- The runner has no PMU (`cycles` is `<not supported>`), but `perf record -e
  task-clock` samples fine, and release builds carry debuginfo, so per-instruction
  annotation works. Ship annotates out as artifacts; the log truncates them.
- A workflow that exists only on a branch cannot be `workflow_dispatch`ed; trigger
  on `push` to the branch instead — push events run the file as of the pushed
  commit.
- `grep -c` finding zero matches exits nonzero and kills an unguarded step; a
  Python `re.sub` replacement string turns `\n` into real newlines. Both broke a
  round each. Splice text with plain string ops, and end probe steps with `exit 0`.

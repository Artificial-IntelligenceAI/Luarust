# Iteration 3

Kept here rather than in a chat, because it has been lost to a context compaction twice.
Fifteen crates, about 25,500 lines of Rust as this is written.

## Done

**The hot JIT** — `[run] mode = "hot"`. Interprets, counts back edges, and when a loop
passes ten thousand compiles what that loop can reach and enters it at the loop head with
the VM's live registers. Takes loops in routines as well as at the top level, keeps the
machine code for a routine that goes hot so later calls land on it, and enters a kept call
in 85 ns rather than 160. `notes/the-hot-jit-enters-mid-loop.md` has the whole shape.

**Ahead-of-time native output** — `luarust native file.lr`. LLVM writes an object, `cc`
links it against `luarust-native`, and what comes out runs on a machine with nothing
installed: no LLVM, no `luarust`, no chunk. 1.6 MB stripped against the toolchain's 56 MB.

The thing that made it work is a crate split. The runtime used to live inside
`luarust-jit`, so linking against it would have dragged thirty-two megabytes of LLVM into
every shipped program. It is `luarust-runtime` now, with no compiler in it at all, and
`luarust-jit` depends on it rather than owning it. LLVM belongs to the machine that
*builds* a program.

## Left

**Inline `$bash { }`, gated on native output.** Only meaningful for a program that is
becoming a binary, because a chunk that runs anywhere cannot promise a shell exists there.
The syntax sketch is `$<language> { ... }`. Open: what the equivalent is per platform, and
what crosses the boundary in each direction.

**External FFI — calling C from Luarust.** Not the internal boundary that already exists
(compiled code calling `luarust_print_text` and the rest of `luarust-runtime`). This is the
language gaining a way to declare a foreign function and call it. Four things have to be
decided before any of it can be written, and three of them are design calls:

- **Which types may cross.** The integer widths and `b32`/`b64` map onto C directly.
  `str`, `er`, arrays, the decimals and `b128`/`b256` do not, and each needs a
  representation decision or a refusal.
- **How each path calls out.** Native code can let the linker resolve the symbol. The JIT
  would resolve it at run time. The VM and the tree-walker have no machine code to put a
  call into at all, so they need either a libffi-style dynamic call or a generated thunk —
  and if they cannot call out, a program using FFI stops being runnable three ways.
- **What it does to the oracle.** This is the sharp one. The whole testing architecture
  rests on three implementations computing the same answer, and the generator writing
  programs whose answers are checkable. A foreign call has effects outside Luarust that no
  path can independently compute, so FFI programs cannot be differentially tested the way
  everything else is. Whatever the design, it should say what replaces that guarantee.
- **What it does to the character of the language.** Luarust refuses to guess, checks what
  it can prove, and reports faults rather than crashing. One `extern` call can corrupt any
  of that, and no amount of checking on this side prevents it. Whether the boundary is
  marked in the source, and how loudly, is a design call rather than an implementation one.

**Parallel loops.** Added 2026-09-01. The counted loop is unusually well shaped for it --
`loop.temp.range.ui64 ['i'] = [|1|, |n|]` has known bounds and a counter the body cannot
assign to, which is what every auto-paralleliser wants and what a `while` never gives.

The grammar already does modifiers with dots, so this is another dot rather than a new
construct, and deleting it leaves the sequential loop:

```luarust
loop.temp.parallel.range.ui64 ['i'] = [|1|, |n|] { ... }
```

That property is not cosmetic. It is what keeps the oracle alive: the tree-walker ignores
the modifier and runs the loop in order, and if the independence the user asserted is real,
all three paths still agree. If it is not real, they disagree and the fuzzer says so --
which is the right failure.

Four things stand in the way, in order of difficulty:

- **The heap is one thread's.** `luarust-core::heap` is a `thread_local!` holding every
  array the program has made, and so are the runtime's frames, output and constants.
  Iterations touching arrays on several cores need it shared and locked, or partitioned.
  That is a different runtime, not an adjustment to this one.
- **Arrays alias.** A handle is a four-byte index and `Op::Move` copies the handle rather
  than the array, so two names can mean one array. Proving two iterations do not touch the
  same element is the hard half of dependence analysis.
- **Reassociation changes answers.** Float addition is not associative, and this language
  prints exact values -- a reordered sum does not round differently, it *prints*
  differently. Under `overflow = "trap"` a partial sum can also trap where the sequential
  one never would. OpenMP's answer is to make the programmer name the operator
  (`reduction(+:sum)`); the other answer is to refuse accumulating loops in the first
  version and allow only ones that write to distinct places.
- **Printing inside one** interleaves differently every run. Either forbid it or order it.

The model to copy is the annotation family -- OpenMP, Fortran's `do concurrent`, Julia's
`Threads.@threads` -- and not the library family (Rayon, `Parallel.For`, Java streams),
which needs closures and iterators that Luarust does not have.

**Threads the programmer allocates.** Raised 2026-09-01, not yet a decision. Worth keeping
apart from parallel loops, because they are different problems wearing similar words.

A parallel loop is *structured*: the parallelism starts at the loop and ends at it, and
nothing escapes. That is why it is tractable, and why the answer stays deterministic. A
thread the programmer allocates is *unstructured* -- it can outlive whatever made it, hold
whatever it was given, and run in an order nobody chose. Every hard thing about concurrency
lives on that side of the line.

For this language the sharp cost is the oracle again, and worse than FFI's. A parallel loop
over independent iterations has one right answer, so three implementations can still be
made to agree. Two threads sharing state have *many* right answers, and a test that insists
three paths print the same thing has nothing left to insist on.

There is a third model that dodges most of it, and the existing design has already half
built it: **isolated tasks that share nothing and pass messages**, as Erlang does. The heap
being a `thread_local!` is exactly the wrong shape for parallel loops and exactly the right
shape for this -- a task gets its own heap for free, values are copied across rather than
shared, and no lock is needed anywhere because nothing is reachable from two places. It
does not restore determinism, but it gives memory safety without asking for any of the
machinery shared-memory threads would need.

So the order, if any of it happens: parallel loops first, isolated tasks second, and
shared-memory threads last if ever, because that one argues with the first line of the
README.

## Not iteration 3

Caching aside, the hot JIT still cannot count *bare calls* — a routine with no loop in it,
called a great many times, never trips anything. Measured and deliberately deferred: an
interpreted leaf call costs 38 ns and entering compiled code costs 85, so a policy that
counted calls without a body filter would compile leaves into something slower.
`crates/luarust-jit/tests/call_cost.rs` is the ruler.

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

## Not iteration 3

Caching aside, the hot JIT still cannot count *bare calls* — a routine with no loop in it,
called a great many times, never trips anything. Measured and deliberately deferred: an
interpreted leaf call costs 38 ns and entering compiled code costs 85, so a policy that
counted calls without a body filter would compile leaves into something slower.
`crates/luarust-jit/tests/call_cost.rs` is the ruler.

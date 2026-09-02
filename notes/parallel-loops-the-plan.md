# `para`: an ordinary loop that goes round on several cores

`loop.temp.para.range.ui32 ['i'] = [|1|, |n|] { … }`. Settled by Tankun on 2026-09-02:
**it is a loop, just parallel** — not a restricted subset of one, and the first version has
to actually use the cores rather than landing the syntax and promising threads later.

## What is done

The word parses. `para` is one more arm in the chain `loop_stmt` already walks, it is
carried on `ast::Loop` as an `Option<Span>` rather than a flag — every later refusal wants
to point at the word that asked — and it is refused on a `while` loop at parse time, since
a loop that does not know how many passes it will make has nothing to hand out.

Nothing acts on it yet, so a `para` loop is an ordinary loop today. That is the property
worth keeping: **deleting the word leaves a program that means the same thing**, which is
what lets the tree-walker ignore the modifier and stay the oracle. Run in order, a parallel
loop is always a valid execution of itself.

## The correction the design turns on

`notes/iteration-3.md` argued that a wrongly-marked parallel loop makes the paths disagree
and the fuzzer says so, "which is the right failure". **That is only true if the parallel
run is memory-safe.** Two threads writing one array element through raw slices is a data
race, which is undefined behaviour — a torn value, a crash, or silence, and no reliable
disagreement. The oracle cannot catch UB; that is the whole reason the register work stayed
inside safe indexing.

So the design has to *make* the note's claim true rather than assume it:

- **Array elements are reached as `&[AtomicU64]` for the duration of a parallel loop.**
  `AtomicU64` and `u64` have identical layout, so this is a view and not a copy, and relaxed
  loads and stores are plain loads and stores on the hardware. A collision then yields *one
  of* the written values rather than a torn one — a wrong answer, which is exactly what the
  tree-walker disagreeing detects.
- **Each worker clones the frame's register file.** So `sum = sum + i` in a parallel loop
  gives every thread its own partial, which is wrong against the sequential run and caught
  the same way. No dependence analysis, no refusals, no promises taken on trust.

## The shape

- `std::thread::scope`, so workers borrow from the parent stack: no `Arc`, no `'static`, and
  the fuzzer's ten-threads-each-with-its-own-heap arrangement is untouched. `luarust fuzz`
  already uses scoped threads, so the pattern is in the tree.
- The heap is the blocker and is why this is not a small change. `luarust-core::heap` is a
  `thread_local!` holding every array; a worker sees an empty one. The atomic views are
  taken on the parent thread before forking and handed down, so nothing about the
  single-threaded path changes and it pays nothing.
- Elements that are not word-shaped — `str`, `er`, `b128`/`b256`, the decimals — are `Rc`
  and `Vec<Bits>`, and there is no atomic view of those. Those loops run in order, correctly.

## Open, and Tankun's to answer

- **The JIT.** A `para` loop under `mode = "whole"` or `"hot"` needs the same treatment in
  compiled code. The honest first version is the JIT running parallel loops sequentially,
  which is correct — and means the VM beats the JIT on exactly these loops, which is a
  strange thing to ship.
- **How many threads.** Hardware concurrency, or a `[run] threads` setting the chunk carries
  the way it carries `mode`.

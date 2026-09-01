# The VM's dispatch loop

A piece of work with a plan needed before code. Written 2026-09-01 after three attempts
that all made it slower, so that whoever takes it does not repeat them.

## Where it stands

    30M iterations, bytecode VM
      loop + add, no array          189 ms      6.3 ns an iteration
      loop + add + xs[i]            423 ms     14.1 ns

    the same loop, Lua 5.5's VM      76 ms      2.5 ns

Six times Lua on array reads, about twice on scalar arithmetic. The scalar benchmark in
the README has the VM at 3.4x C, which is respectable; this is the part that is not.

## What has already been taken

Do not spend time on these; they are done and in the history.

- `Op::At` built an `out_of_range` fault -- formatting a message, allocating a `String` --
  for **every element that was fine**, because `ok_or` evaluates its argument. 3,385 ms to
  651. The tree-walker had `ok_or_else` on the same line, which is why it was *beating* the
  VM on array code and nobody noticed.
- The array's shape was looked up per element, through a thread local and a `RefCell`, for
  something that cannot change while a program runs. Resolved once at widen time. 569 to
  478.
- Doing that by folding the dimensions into the instruction took `Micro` from twelve bytes
  to twenty, and *every* instruction fetch loads one -- so the arithmetic paid for a field
  only array reads use. It carries the shape's index instead. A test now holds `Micro` to
  `Op`'s size.
- `Value` was twenty-four bytes because `Rc<str>` is a fat pointer, and the widest variant
  sets the size of every value in the language. `Rc<String>` is thin; sixteen now.

## The constraint, which is the whole reason this needs a plan

`run_with` is one very large function and every arm shares its registers. Adding a
number-only fast path to the arm that reads an array made **the arm that adds two numbers
seventy-seven per cent slower**, with nothing about addition changed. Measured twice, once
with and once without the `continue` that looked like the culprit.

So anything that *grows* that function is paid for by every instruction in the machine.
That rules out, as they were sketched:

- superinstructions -- more arms
- typed fast paths per element kind -- more arms
- anything that pattern-matches more deeply inside an arm

Moving one arm out behind `#[inline(never)]` was tried too and came back ten per cent
slower, which is inside the noise below and so is not evidence either way. A *systematic*
split -- a small hot loop over the few opcodes that matter, every cold one outlined -- has
not been tried and is a different thing from outlining one arm.

## Measuring it is harder than it looks

Two builds of **identical source**, `git status` clean, best-of-six interleaved runs:
189 ms and 204 ms. Eight per cent, from code layout alone.

So a change worth less than about ten per cent cannot be told from a rebuild with that
method, and several of this evening's smaller numbers should be read with that in mind.
Comparing the *same binary* over time is reliable -- interleave the two, take minima, and
the distributions separate cleanly. Comparing two builds is not.

Whoever takes this should fix the instrument first. Options, roughly in order of effort:
run the benchmark in-process so one binary measures both arrangements; use hardware
counters rather than wall clock; or take many more samples and compare distributions
rather than minima.

**Fixed, the first way.** `run_widened` in `luarust-vm` takes the arrangement as a
parameter, and `tails_fused_against_not` (ignored, `--nocapture`) times both from one
binary, interleaved, minima — layout cancels because there is one layout. Its unfused
column reproduced the 6.3 ns baseline to a twentieth of a nanosecond on its first run,
which is the calibration the old method could not have shown. The pattern generalises:
a change worth less than the 8% build noise must be runtime-switchable long enough to
be measured, and only then hardwired.

## Directions that have not been tried

- **Fusing the loop tail** — tried, kept: `Micro::Tail` folds the counting loop's
  `jump.eq / add / jump` into one fetch and dispatch, only where nothing jumps into the
  swallowed pair, never under a tier (the back edge carries the counter there), the
  swallowed instructions left in place so every index survives. Add loop 6.35 to
  3.71 ns an iteration; array loop 17.48 to 15.33. The constraint held in reverse, as
  predicted: an arm that *removes* dispatches paid nothing — the unfused path in the
  same binary did not move.
- **A small hot loop.** Keep the arms that dominate real programs -- arithmetic, the
  jumps, `move`, `const` -- and outline everything else. The opposite of what was tried:
  outline the *cold* many, not the hot one.
- **A compact encoding.** `Micro` is twelve bytes because it is a Rust enum with a
  discriminant and the widest variant's fields. Lua's instructions are four bytes.
- **Threaded dispatch.** The usual answer, and stable Rust has no computed goto; `become`
  is unstable and the function-pointer-table shape does not reliably become jumps. Worth
  measuring rather than assuming, since the `Micro` split already captured some of it.
- **The range analysis proving indices in range**, which is the one target *outside* this
  loop: it removes the bounds check, and it is the same proof the JIT's vectoriser is
  missing. `crates/luarust-check/src/range.rs` already proves a dividend non-negative and
  rides that to the JIT in a flag on `Binary`; an index would ride the same way.

That last one is the safest place to start, because it does not touch the loop at all.

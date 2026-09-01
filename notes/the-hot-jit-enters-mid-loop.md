# The hot JIT, and how it gets in

`[run] mode = "hot"` interprets a program, counts how often each loop goes round, and when
one passes ten thousand compiles what that loop can reach and jumps into the middle of it
with the registers the VM was holding. That last part is on-stack replacement, and it
is the only part of a tiering engine that is genuinely hard: everything else is a counter.

## Why it enters at a loop head and not only at a function

Compiling hot *functions* is the ordinary design and it is simpler -- a function is entered
afresh, so there is no running state to hand over. It would also do nothing at all for
Luarust as people write it. The benchmark in the README is a loop at the top level, and so
is most of what anybody writes in a small language: `main` is entered once, no counter ever
trips, and a function-granularity hot JIT would sit there watching an interpreter run for
an hour. So loops are counted -- in the top level and inside routines both -- and the join
happens in the middle of one.

## What made it small

Three things already in the code, none of them put there for this:

- **The VM's two-loop shape.** The outer loop settles what code the current frame runs and
  the inner one steps instructions. That outer loop is the tier boundary, already written,
  already there for the reason that made it fast.
- **`blocks::leaders`.** The JIT already gives every jump target its own basic block, and a
  loop head is a jump target. Entering at a loop head is `branch made[&at]` instead of
  `branch made[&0]` -- one expression.
- **The frames are the same shape.** The JIT's cells and the VM's registers are both a
  `Vec<Value>` of `chunk.registers`, indexed the same way. So handing the frame over is
  handing the vector over. `cell_bits` then loads each one into the stack slot compiled
  code reads it from, once, in the entry block.

## What the join has to get right

- **The registers.** Coming in at nought, every register is dead: the checker has proved
  each is written before it is read. Coming in at a loop head, everything the program did
  before the loop is live and has to arrive intact -- narrow floats, wide ones, decimals,
  rationals, strings, bools, array handles.
- **The heap.** `begin` clears it, because a run starts with nothing. `resume` must not:
  the arrays in there are the VM's, and the program is still using them.
- **The clock.** `time` measures from when the program started, not from when a loop got
  hot. The `Instant` is handed across with the registers.

## Why the VM cannot call the JIT

The JIT reads chunks, so it depends on `luarust-vm`; a dependency the other way would close
the circle. So the VM takes a `Tier` and the CLI implements it -- which is also what keeps
`luarust-run` at a few hundred kilobytes. A runtime with no compiler in it installs no tier,
never counts a back edge, and runs a chunk asking for `"hot"` on the VM without comment.

## Loops inside routines

These are counted too, and they are the harder handover, because this one comes back. The
top level runs to the end of the program; a routine runs to its `return`, hands the answer
over, and the VM pops the frame and carries on interpreting the call underneath, which
never stopped waiting.

Compiled code entered inside a routine puts that routine's body in `luarust_main` rather
than in the function beside it, because it is entered from Rust and not from a call. It is
given the answer pointer `Op::Return` already writes through, so the return path is the one
that was there. No `cells_enter`: the frame is the live one the VM handed over, and
entering a fresh one from a template would throw away the thing this came to continue.

**Every open frame is handed over, not just the hot one.** They are the root set a
collection inside compiled code walks, so leaving the callers out would free an array only
a caller could still reach. They are also what `call_depth` counts, so a program that runs
out of stack does so in the same place it would have on the VM.

## Only what the loop can reach

Every call names its target index and the language has no function pointers, so the set of
routines a run can reach is exact rather than a guess. Asked from inside one particular
loop, the answer is usually much smaller than the whole chunk -- and the instructions
before the loop are usually unreachable too.

    forty routines, none of them hot          whole 47.7 ms    hot  8.6 ms
    forty routines, one called three million  whole  154 ms    hot  120 ms

The second row is why this matters beyond startup: `"hot"` compiles one routine where
`"whole"` compiles forty, so it wins outright on a program where only part of the code is
worth compiling.

"Before the entry" and "unreachable" are not the same thing, which is the one place this
could have gone quietly wrong. Entering at an inner loop's head, the outer loop's back edge
lands *behind* the entry, and everything the outer loop does is live. `blocks::reachable`
follows the graph rather than comparing instruction numbers, and `blocks.rs` has that case
as a test.

## What it does not do yet

- **Compiled code is thrown away after the activation.** A counter fires once, the module
  is built, one activation runs on it, and the module goes. So a routine that is hot
  because it is *called* a great many times -- rather than because one call goes round a
  great many times -- gains almost nothing: the one activation that tripped the counter
  runs compiled and every later call is interpreted again. Caching compiled routines and
  dispatching calls to them is the next step, and it is what the cumulative counter is
  already measuring the right thing for.
- **A counter fires once.** If the JIT declines, that loop is never asked about again.

## Testing it

`crates/luarust-jit/tests/four_ways.rs` runs a program on the interpreter, the VM, the
whole-chunk JIT and the tiering engine, and insists all four agree. The threshold is a
`Tier` method rather than a constant precisely so a test can set it to one -- at ten
thousand no generated program would ever switch, and the point is to make the join happen
in as many different places as possible. `twenty_thousand_agree_four_ways` is the deep
version, behind `--ignored`.

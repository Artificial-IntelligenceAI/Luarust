# The hot JIT, and how it gets in

`[run] mode = "hot"` interprets a program, counts how often each loop goes round, and when
one passes ten thousand hands the whole thing to LLVM and jumps into the middle of that
loop with the registers the VM was holding. That last part is on-stack replacement, and it
is the only part of a tiering engine that is genuinely hard: everything else is a counter.

## Why it enters at a loop head and not at a function

Compiling hot *functions* is the ordinary design and it is simpler -- a function is entered
afresh, so there is no running state to hand over. It would also do nothing at all for
Luarust as people write it. The benchmark in the README is a loop at the top level, and so
is most of what anybody writes in a small language: `main` is entered once, no counter ever
trips, and a function-granularity hot JIT would sit there watching an interpreter run for
an hour. So loops are counted, and the join happens in the middle of one.

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

## What it does not do yet

- **A hot loop inside a routine is not noticed.** Taking over the top level means running
  to the end of the program and never coming back; taking over a routine means returning
  into the middle of a call the VM is holding. Only the top level is counted, so nothing is
  spent looking for something that cannot be acted on.
- **It compiles the whole chunk**, every routine included, not just the loop that got hot.
  The saving is that a program which never gets hot never pays LLVM at all -- not that a
  program which does pays less.
- **A counter fires once.** If the JIT declines, that loop is never asked about again.

## Testing it

`crates/luarust-jit/tests/four_ways.rs` runs a program on the interpreter, the VM, the
whole-chunk JIT and the tiering engine, and insists all four agree. The threshold is a
`Tier` method rather than a constant precisely so a test can set it to one -- at ten
thousand no generated program would ever switch, and the point is to make the join happen
in as many different places as possible. `twenty_thousand_agree_four_ways` is the deep
version, behind `--ignored`.

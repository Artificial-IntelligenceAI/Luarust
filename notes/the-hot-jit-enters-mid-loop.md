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
the circle. So the VM takes a `Tier` and something above both implements it. A build with
no compiler in it installs no tier, never counts a back edge, and runs a chunk asking for
`"hot"` on the VM -- which is what keeps a runtime at a few hundred kilobytes.

**Two details of that changed on 2026-09-02 and this section used to say otherwise.**

`Compiling` lived in the CLI, on the reasoning that it was the one place holding both
halves. The circle is still real; the other half of that reasoning -- that the JIT has no
business driving the VM -- was a preference, and it stopped being affordable when a second
binary wanted a hot loop taken off it. It lives in `luarust-jit` now, which is the only
crate that has to exist for either caller to work.

That second binary is `luarust-run --features jit`, off by default. A runtime that does not
ask still has no compiler in it at all; what the feature buys is that shipping a program
which wants `"hot"` means shipping a *runtime* rather than the toolchain. It is worth more
than it looks: stripped, the runtime is 0.6 MB, the runtime with a JIT 42.7 MB, and the
toolchain with a JIT 100.2 MB -- because `write_object` calls `Target::initialize_all` so
`luarust native` can build for a machine that is not this one, and the toolchain therefore
links a code generator for every architecture LLVM has. A runtime compiles for the machine
it is standing on and wants exactly one of them.

And it no longer happens *without comment*. A build with no JIT now says once, on `stderr`,
that the chunk asked for an engine it has not got and is running on the VM. Refusing to run
would help nobody; running several times slower than asked and saying nothing is a
different thing, and it was only noticed because `luarust jit` on the same build explains
itself at length while a chunk asking for the same thing got silence.

## What "compile once, run anywhere" does and does not cover

The chunk is compiled once and runs anywhere. The *machine code* is not: there is no
cross-run cache anywhere in `luarust-jit`, and `Compiling.kept` is a `Vec` in the process
that goes when the process does. So a `"hot"` program compiles from bytecode to machine
code on every single run, on every machine, and throws it away at exit. Twenty thousand
iterations of an add loop: 4.4 ms on the VM, 7.0 ms hot, and the difference is LLVM being
paid again.

Which makes the three properties pick-two. `"vm"` compiles once and runs anywhere and is
not fast. `"hot"` runs anywhere and is fast and recompiles every run. `native` compiles
once and is fast and runs on one target -- the README says as much, that it "trades the
anywhere for a binary that needs nothing on the machine it lands on."

A cross-run cache keyed on the chunk's checksum -- FNV-1a, already in the file and already
verified before any field is believed -- would give `"hot"` the third property. Nobody has
asked for it and it is a real piece of work, not a footnote.

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

## Routines are kept

A routine that goes hot is compiled twice: once in the resumed shape that serves the
activation that tripped the counter, and once in the entered shape -- at instruction
nought, on the fresh frame a call builds -- which is kept. Every later call the VM
would have interpreted lands on the kept code instead: the VM asks `Tier::keeps`
before pushing a frame, which costs a lookup, and hands the frames over only on a yes.
The interpreted call and the kept one end identically because the answer comes back
exactly as `Taken::Returned` carries it.

Where that pays is the shape the resumed module cannot help: an outer loop below the
threshold calling a routine whose inner loop is hot. The activation that trips is
served once, and before keeping, every one of the thousands after it was interpreted:

    5,000 calls, 12,000 iterations each      before 742 ms    kept 160 ms    whole 163 ms

Parity with `"whole"`, still compiling two routines out of forty-one. The LLVM context
behind each kept routine is leaked deliberately -- the cache lives until the program
ends, and machine code whose context was dropped is a dangling pointer.

The entry itself has since been slimmed, in the order the risks deserved. The two
harmless costs went first: a module's constant and template tables are installed once
and re-entered on a token, and the output buffer is written and flushed only when
something is in it — 160 ns to 99 on the `call_cost` ruler. Then the risky one: the
caller's open frames are *borrowed* for the span of the call rather than cloned, as
raw pointers the runtime holds from `reenter` to `retire` and reads only as collection
roots and depth — 99 to 85 on a one-frame harness, and O(1) in stack depth where the
clone was O(everything held). Because a mistake in a shared root set corrupts rather
than crashes, the change carries its own guards instead of leaning on the fuzzer: a
debug assertion that every borrowed frame is exactly where and how long it was when
the call began, and `kept_calls_survive_aggressive_collection`, which sweeps across
the handover on nearly every kept call and stares at the values only a suspended
caller still names.

## What it does not do yet

- **A counter fires once.** If the JIT declines, that loop is never asked about again.
- **Nothing counts bare calls.** A leaf routine with no loop in it never trips any
  counter however often it is called, so it is never kept. Counting calls as well as
  back edges is the remaining half of "hot because it is called" — but measured, it
  is blocked on something else first. The VM makes a whole interpreted leaf call in
  38 ns; one call *into kept code* costs 151 ns before the body runs, because every
  call re-installs the module's constant and template tables, clones every open
  frame for the root set, and round-trips the output buffer. So a policy that
  counted calls and kept everything hot would compile leaves into code that runs
  them four times slower — which rules out counting *with no body filter*, and is
  no argument against counting with one. The case only call counting can ever catch
  — a long straight-line body, no loop, called a great many times — remains real,
  uncaught, and unmeasured. But 151 ns is the wrong constant to design a policy
  around, because it is three removable costs, so the slim re-entry came first —
  all three are done, and the number that survived is 85 ns on a one-frame harness,
  most of it the fresh frame's own allocation. A policy now gets designed against
  that, and it still wants a body filter: 85 ns of entry against a 38 ns interpreted
  leaf call still loses. Both numbers re-derive: the entry cost is
  `cost_of_one_kept_call` in `crates/luarust-jit/tests/call_cost.rs` (ignored, run
  by hand), and the 38 ns is the two programs in its module comment, on the plain
  VM, at N=3,000,000. For the routines the cache already keeps, the 151 ns sits
  under bodies that save thousands, which is why it was never visible.

## Testing it

`crates/luarust-jit/tests/four_ways.rs` runs a program on the interpreter, the VM, the
whole-chunk JIT and the tiering engine, and insists all four agree. Its tier keeps
routines the way the CLI's does, so the sweeps also cover calls landing on kept code,
and one test insists a kept routine actually served a call rather than merely existing. The threshold is a
`Tier` method rather than a constant precisely so a test can set it to one -- at ten
thousand no generated program would ever switch, and the point is to make the join happen
in as many different places as possible. `twenty_thousand_agree_four_ways` is the deep
version, behind `--ignored`.

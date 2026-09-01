# Should the VM be rewritten in C? No — and here is the measurement that says so

One line: the thing you would rewrite in C for — computed goto — is worth nothing on
this machine, and the gap that *is* real between a C interpreter and a Rust one is a
bounds check on every register, which Rust can drop without becoming C.

## The question

The VM's remaining distance to Lua 5.5.1 looked dispatch-shaped, and dispatch is the one
place stable Rust genuinely cannot say what C says: there is no `&&label` / `goto *`, and
`become` is unstable. Lua's VM is built on computed goto. So: rewrite the VM in C?

That claim — "the residual is dispatch" — was inference from how Lua is built, never a
measurement. This note is the measurement.

## The spike

Nine interpreters over the same bytecode, the same eight-byte instruction, and the same
register model the VM has since the unboxing: raw `u64` words indexed by a `u16`. Two
programs, chosen to bracket the answer:

- **thin** — an add and a fused loop tail, two dispatches an iteration. Dispatch is the
  largest possible fraction of the work, so this is the *upper bound* on any dispatch win.
- **fat** — an array read, an add and the tail, three dispatches. Fatter ops, so dispatch
  matters less, and the win shrinks toward what real code would see.

**Every variant runs in its own process.** The first attempt ran all of them in one, and
adding a variant moved the timing of *unchanged C* by sixty per cent: nine interpreters
in one process share an indirect-branch predictor and one code layout, and they alias.
Best of eleven, interleaved, then rebuilt from scratch and re-run twice more — the
percentages moved by a few points across builds and the ordering never did.

|                              | thin, 2/iter | fat, 3/iter |
| ---------------------------- | -----------: | ----------: |
| rust, match in a loop        |           -- |          -- |
| rust, match, unchecked regs  |      −25.1 % |     −11.6 % |
| rust, flat instruction only  |       −8.7 % |      −3.4 % |
| **rust, both**               |  **−28.6 %** | **−14.6 %** |
| rust, registers masked       |       +7.7 % |      +3.3 % |
| rust, fn-pointer table       |      +89.3 % |    +126.6 % |
| C, switch in a loop          |      −26.0 % |     −12.7 % |
| C, computed goto             |      −26.9 % |     −10.5 % |
| C, musttail threading        |      −15.6 % |      −5.7 % |

## What it says

**Computed goto buys nothing.** It ties a plain `switch` on the thin program and loses to
one on the fat program. The historical win came from replicating the dispatch jump so a
weak predictor got per-handler context; the predictor here does not need the help.

**Guaranteed tail calls are worse than both.** `musttail` threading — the variant with the
best theory behind it, since each handler gets its own register allocation instead of one
enormous function spilling state — came third of the three C shapes, in every run.

**C's advantage is real and it is entirely recoverable in Rust.** Rust with the register
file read unchecked and a flat instruction ties or beats every C variant. Decomposed, the
gap is a bounds check on every register access (the larger half) and taking a Rust enum
apart rather than reading named fields (the smaller half). Neither is about the language.

**Two ideas that looked free and were not.** Masking the register index to a power-of-two
frame drops the bounds check with no `unsafe` and no trust — and is *slower* than the
check it removes, because the `and` lands in the address dependency chain while the check
predicts perfectly. And a table of function pointers, stable Rust's nearest reach at
threading, is worse than twice as slow: stable Rust cannot thread, it can only call.

**The number that limits every number above.** The spike's ops are far thinner than the
VM's — 1.55 ns an iteration here against 4.51 in `run_with`. Dispatch and register access
are a larger fraction of this harness than of the real machine, so each figure is a
ceiling on what the VM could gain, not a forecast.

## What was done

Registers in `Split` are read and written through `word`/`set_word`, which do not bounds
check. This is not a new promise. `serialize::check` already says the VM "indexes
registers, constants, text and instructions without checking, because the compiler never
produces an index that is wrong", and holds every register of every instruction against
its own routine's count before a loaded chunk is handed back — so the index is proved once
per instruction in the file instead of once per instruction executed. The loop was paying
the second one anyway.

The fused tail read the jump beside it with `let Micro::Other(Op::Jump { target }) = … else
{ unreachable!() }`, which is a tag test and a panic path on the hottest instruction in the
language, for a tag `widen` decided when it built the thing. It says `unreachable_unchecked`
now.

End to end, `luarust run`, best of nine interleaved against a binary built from the commit
before:

| | before | after | |
| --- | ---: | ---: | ---: |
| scalar, 100M | 450.9 ms | 402.7 ms | **−10.7 %** |
| array, 30M | 342.9 ms | 310.1 ms | **−9.6 %** |

Both clear of the ~8 % build-to-build noise this machine has, and best and median agree
to within a point.

## What was left, and why

**The flat instruction was not taken.** Worth −8.7 % and −3.4 % on its own in the spike,
with no `unsafe` at all, but it means `Micro` stops being a Rust enum, which is a change
to how every arm of the dispatch loop is written and how `widen` builds one. Worth doing
deliberately, not as a footnote to this.

## The instruction fetch: a rule worth having, for a reason that turned out to be wrong

This note first said the fetch kept its bounds check only because a chunk could run off
the end of its own code — `check` proves every jump target lands somewhere real and that a
`Halt` exists, but never that control reaches it. That was verified, not reasoned: `Const
r0`, `Jump → 3`, `Halt`, `Move r0, r0` passed every check, loaded without complaint, and
panicked reading instruction four of four. It broke the promise `serialize` makes in its
own first paragraph, that a corrupt file produces a complaint and not a crash.

So the rule went in. `Ends` says the top level must end in a halt and a routine in a
return, which is enough to make walking off the end impossible, and refuses nothing
anybody meant: 5,010 compiled chunks and 8,785 routines, every generated program and every
example in the tree, all already ended that way.

**And then the fetch was left checked anyway.** With the rule in place `at` provably names
a real instruction, so `get_unchecked` is sound — and measuring it found nothing: −2.7 %
by best and +1.6 % by median on the scalar loop, +0.4 % on the array one. Signs that
disagree are a measurement of noise. The check is a compare against a value already in a
register, on a path whose load dominates it, and the predictor never misses.

Which is the honest correction to make: the rule is worth having because a chunk that
crashes the VM is a broken promise, not because it buys speed. It buys none. The `unsafe`
that would have collected it is not written.

## Afterwards: the array loop, and two more guesses that measured nothing

The scalar parity this note's numbers led to was partly the benchmark's doing. The loop
everything was measured on computes `(sum + i) mod 1000000007`, and a modulo dominates
both machines and hides what is underneath. Take it out and Lua is 1.68 ns an iteration
against 2.63 -- **1.56x**, not the 1.03x the division-bound loop reports. Measure a second
shape before believing a headline; the same lesson the `ui64`-against-signed mix-up taught
once already.

The array loop was 4.04x. Two guesses at why -- the index arithmetic, then the shape
lookups -- were both written, built and measured at 0.0%. Ablating the arm a piece at a
time is what answered it:

| the element read | ns |
| --- | ---: |
| as it stood | 10.42 |
| with the index computation removed | 8.92 |
| with the element load removed | 8.02 |
| with the whole arm stubbed to a store | 6.18 |

The last row is the finding: 6.18 ns with the array read gone entirely, against 2.63 for a
plain add loop. `dis` said why -- a `move` copying the loop counter into a temp before
every `array.at`, a whole dispatched instruction an element. And it is there on purpose:
`compile::arguments` records that removing it makes the VM ten per cent faster and stops
LLVM vectorising the compiled loop, worth thirteen times, tried and put back.

So the chunk keeps it and `widen` fuses it away for the machine alone -- `Micro::AtOne`,
one dimension, nothing else arriving at the `At` it swallows. **307.6 -> 200.9 ms, -34.7%**,
and `tests/optimised.rs` still finds the vector unit.

## The standing answer

C is not on the table for the VM. If it comes back it should come back for something C is
actually better at — and dispatch, on this hardware, is not it.

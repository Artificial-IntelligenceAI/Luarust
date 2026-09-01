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

**The instruction fetch keeps its bounds check.** `check` proves every jump target lands
on a real instruction and that a `Halt` exists *somewhere* — not that control ever reaches
it. A chunk ending in something that is not a stop runs off the end of its own code. That
is a panic today; with the fetch unchecked it would be worse than a panic, so the fetch
stays honest until the format has a rule that the last instruction stops.

**Verified, not reasoned.** These four instructions — `Const r0`, `Jump → 3`, `Halt`,
`Move r0, r0` — pass `check`, pass `well_typed`, load without complaint, and then panic
with `index out of bounds: the len is 4 but the index is 4`. Which is also a hole in a
promise the serializer makes in its own first paragraph: that a corrupt file produces a
complaint and not a crash. A panic is a crash.

**The flat instruction was not taken.** Worth −8.7 % and −3.4 % on its own in the spike,
with no `unsafe` at all, but it means `Micro` stops being a Rust enum, which is a change
to how every arm of the dispatch loop is written and how `widen` builds one. Worth doing
deliberately, not as a footnote to this.

## The standing answer

C is not on the table for the VM. If it comes back it should come back for something C is
actually better at — and dispatch, on this hardware, is not it.

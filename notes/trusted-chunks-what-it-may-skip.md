# `[run] chunks`: settled, and the line it may not cross

Settled by Tankun on 2026-09-02, after he pushed back on a recommendation to drop it.
`notes/vm-registers-unboxed-design.md` is where the idea started; this is what it became.

## Settled

**Keep it, and let it travel in the chunk, the way `[run] mode` does.**

The argument for dropping it was that the VM's job is to be the honest second opinion —
`Op::Binary` carries `nonnegative` as *advice for a compiler*, and its own comment says
the JIT drops guards on the strength of it "while the VM ignores it and computes floored
`mod` the long way, so a proof that was wrong is caught by the paths disagreeing rather
than believed by both." Two holes in that:

**The precedent is stronger than the objection.** Trust was said to differ from
`overflow`, `division` and `floats` because those change what a program computes and trust
does not. But `[run] mode` does not change what a program computes either, and it travels
in the chunk as `Engine`, because `luarust-run` has no project file and never looks for
one. A project setting applies to the project, and the chunk is what the project ships.

**"The VM is the second opinion" only holds during the gate.** Three paths on one program
is what makes a bad proof visible. In production a user runs one. A VM that re-derives a
proof catches nothing there — it only declines to act on a claim nobody verified, which is
a defence against a malformed chunk and not the property the objection was selling.

## The line

> Trusted may skip a check whose failure is a wrong answer or a vaguer error message. It
> may never skip a check whose failure is a read outside an allocation.

More generous than it sounds, because the array path already has two nets. `offset` holds
an index against the shape's dimensions; `heap::read_bits` then does `v.get(at)` on the
real store whatever `offset` concluded, and `load_element` turns a miss into a fault.
`offset`'s own comment records that it stopped checking the real length *because the heap
was about to*. So a trusted chunk may drop the dimension check and stay memory-safe; what
it gives up is "index 5 of an array of 3" in favour of something vaguer. That is where to
start, and the whole of what is known to be safe today.

## How big it is, honestly

Small. Not a headline, and smaller than the first draft of this note claimed.

That draft argued the niche was a deployment which wants **one portable `.lrc` rather than
a per-target build matrix**, on the grounds that reaching the JIT's speed otherwise meant
carrying thirty-two megabytes of LLVM to a stranger's machine. Tankun's answer to that was
that 32 MB is about eight photographs, and nobody has ever declined software over it. He
is right, and it took four goes to get there — the claim started as "`hot` cannot travel
to a stranger's machine" and shrank every time it was checked:

- **`hot` travels.** LLVM is statically linked; the only dynamic dependencies of a
  JIT-capable binary are `libzstd` and the system `libc++`. Nothing needs installing.
- **LLVM was not the reason.** It was a packaging decision — `luarust-run` had no `jit`
  feature and no way to gain one. It has one now.
- **The front end was not the weight either.** It is 712 KB of a 32 MB binary, about two
  per cent.
- **And 32 MB is not a burden.** Node is 50–90 MB and a JDK is 180.

So the honest case for `[run] chunks` is not that a portable deployment is *forced* onto
the VM. It is that it *chooses* the VM — one file, 461 KB to run it on, no build matrix —
and the setting is what makes that choice a little cheaper. Worth a bounded amount of
work, and no more.

## Measure it first, and not by reading the code

`offset` cost **1.50 ns an element when the array loop was 10.42**. That loop is **6.40**
now — `Micro::AtOne` took 34.7% off it and the specialised arithmetic another 9.4% — so
the figure this feature was justified by is stale, and the fraction it represents is
larger than it was. Re-measure against today's `main`, **by ablation**: three diagnoses
reasoned from reading the code this week each measured 0.0%, and every ablation found
something. `notes/dispatch-was-never-the-language.md` has that story at length.

## Still open

The key name. `chunks = "checked" | "trusted"` is what the design note used and nothing
has been named for real yet; Tankun picks it when there is something to attach it to.

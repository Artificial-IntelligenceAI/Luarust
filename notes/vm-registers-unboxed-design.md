# Unboxing the VM's register file: the design, before the code

One line: registers become raw `u64` words with the type taken from the opcode, only
genuinely heterogeneous values keep a `Value` cell beside them, and the split is not
invented — it is `celled(ty)`, the one the JIT already runs on the same value model
through the same differential tests.

## The argument

Every register move today carries a 16-byte `Value` whose `needs_drop` is true, so
moving an `i64` pays a discriminant branch to decide whether an `Rc` wants touching.
The bytecode names the type on every instruction — `add r6, r0, r1 -- i64` — so the
tag in the register is a second copy of something the dispatch loop already knows.
The compiled path proved this out independently: `celled(ty)` in `luarust-jit` keeps
`B128 | B256 | Str | Er` and the decimals in cells and everything else — integers,
`b32`/`b64` bits, `bool`, array handles — as raw words in machine registers, and it
passes the same fuzz and four-way gates the VM does. The VM adopting the split is
convergence, not speculation.

Fits the constraint that killed three attempts: this makes the hot arms smaller —
drop glue gone, discriminant branch gone, half the traffic — rather than adding any.

## The shape

- A frame holds `raw: Vec<u64>` and `cells: Vec<Value>`, both `registers` long, the
  slot-and-cell-beside-it layout the JIT's emitter already uses. An instruction whose
  type is celled works the cells; every other reads and writes raw words. Which side
  an operand lives on is decided at widen time from the instruction, never at run
  time from a tag.
- Collection roots are the cells plus mirrored handles: an array handle is a raw
  word, so allocation sites mirror it into the cell beside its register, exactly
  `cell_holding` in the JIT. Root walks read cells only. Allocation sites are cold
  next to arithmetic; the mirror is their cost, not the loop's.
- v1 unboxes only when no tier is installed, the same gate fusion uses. `luarust
  run` and the runtime-only build get it whole; `"hot"` keeps `Value` frames, so the
  `Tier` handover keeps its shape and the OSR join is untouched. Teaching the
  handover to rebuild `Value` frames from raw+cells needs the per-PC type map below
  and is its own step.

## The question the design has to answer, not dodge

The VM's `not_as_described` fault (R0016) exists because a chunk can arrive from
anywhere and lie: today the tag in the register catches an instruction whose type
disagrees with what the register holds. Raw words have no tag, so that net vanishes
for every unboxed type — a lying chunk would compute nonsense instead of stopping.

The principled replacement is to move the check from run time to load time: a widen
pass that infers, per program point, the type each register holds — the checker's
"an instruction's type is the type of the values it works on", verified over the
chunk instead of trusted per instruction. A chunk that types inconsistently is
refused as `Broken`, the way every other malformed chunk is. That is *stronger* than
today's net (it refuses programs whose lie never executes), it is the same pass the
tier handover will want later, and it keeps the rule that a loaded chunk proves
itself. Sized honestly: it is dataflow over register types with joins at jump
targets, not a large piece, but it is the prerequisite, not an add-on.

## Measuring it

The fused-tail rule generalises: anything under the 8% build-layout noise must hold
both arrangements in one binary. Here that is a generic parameter on the inner loop
(two monomorphisations, one build) or keeping the `Value` loop compiled alongside,
measured by the same interleaved-minima harness beside `tails_fused_against_not`.
Expectations kept modest on purpose: the in-place `Num` write already dodged most of
the drop glue on the arithmetic path, so the win is the branch and the traffic, and
the 77% surprise says predictions about this loop are worth little until the
instrument speaks.

## Where the numbers stood when this was written

Add loop 3.71 ns an iteration against Lua 5.5's 2.52; array loop 10.98 (be's
build-to-build number on the merged tree) against the same 2.52. The Lua target is
`/opt/homebrew/bin/lua` -> Cellar/lua/5.5.1, Homebrew's default build, untuned — a
`-O3 -march=native` Lua would move the target.

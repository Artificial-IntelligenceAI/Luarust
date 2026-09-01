# The range analysis: what it proves, where it rides, and the twist at the end

One line: the checker now proves, per division, that the dividend cannot be negative
and the divisor cannot be zero or less — the proof rides the chunk as one flag, the
JIT spends it, and the signed benchmark loop now compiles to the exact code the
unsigned one always got: 227 ms on the M5, where the brief hoped for 242.

## The twist worth remembering

The brief asked for removing the floored-`mod` correction (`icmp` + `select` around
`srem`). Doing only that left 251 ms against the unsigned loop's 227, because LLVM's
*signed* divide-by-constant lowering carries its own fixup instructions that no
correctness argument can remove — it must handle negative dividends the proof says
cannot arrive, and LLVM cannot see the proof across the loop's phi. The move that
closes the gap completely: with both operands proven non-negative, signed and
unsigned bit patterns coincide, so emit `urem`/`udiv` outright. Same values, cheaper
lowering, and the guards and the correction all gone in one stroke.

## Design decisions, all Tankun's, none to re-open

- Analysis in `luarust-check` at ir level (a range loop declares its counter bounds;
  chunk code would need loops pattern-matched back out of jumps).
- The fact travels as `nonnegative` on the chunk's `Binary` — VERSION 12 -> 13, an
  older `.lrc` simply lacks it and keeps the guards.
- The JIT is the only consumer. The interpreters ignore the flag and compute floored
  `mod` the long way, so the three-way agreement stays the net that would catch a
  wrong proof.
- Accepted knowingly: the flag is the one chunk field `check()` cannot validate — a
  proof indexes nothing. A file that lies here makes the JIT silently compute a
  truncated remainder where floored differs. Said plainly at the decode site.

## Two analysis subtleties that cost a test failure each, or would have

- The *range* of a floored remainder needs only the divisor's sign ([0, d-1] against
  a positive divisor, any dividend); the *flag* needs the dividend too. Conflate them
  in the flag's direction and the proof is wrong. There is a test whose whole job is
  this distinction (`a_counter_from_below_zero_is_not`).
- Widening must not forget the sign. `overflow = trap` clips an escaping interval to
  the type (the runs that continue held a value that fit); `wrap` widens to the whole
  type. And when the loop fixpoint gives up chasing a growing interval, it jumps to
  "at or above zero, any size" when every observed bound stayed there — then runs an
  extra pass to *verify* the guess, demoting to unknown whatever will not hold still.
  Without that, `trap` programs lose the one bit the pass exists to keep.

## Verification

Beyond the standing gate (tests, three clippys, `fuzz 200000`), the change grew
`deep_agreement.rs` in the JIT's tests: 200,000 generated programs, three ways each,
`#[ignore]`d out of the ordinary gate and run by hand for changes to what the JIT
emits — because 3,000 has already let a bug through once where 200,000 would not.

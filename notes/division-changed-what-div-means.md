# `div` changed, and old chunks say so

Until chunk version 14, `div` truncated and `mod` floored. Those are two different
divisions, so the identity every quotient-and-remainder pair is supposed to satisfy did
not hold:

```
-7 div 3 = -2   and   -7 mod 3 = 2    ->   (-2 x 3) + 2 = -4, not -7
 7 div -3 = -2  and    7 mod -3 = -2  ->   (-2 x -3) + -2 = 4, not 7
```

All three paths agreed on it. That is the whole lesson: the fuzzer compares
implementations to each other, and never to arithmetic, so a wrong answer all three
give is invisible to it. `crates/luarust-jit/tests/division.rs` compares to a quotient
and a remainder worked out from first principles instead, and asserts the identity
directly.

The fix made the rounding a project setting — `[defaults] division`, one of `floored`
(the default), `truncated` or `euclidean` — because all three are defensible and the
project should say which it wants. `div` and `mod` now come out of one place, so they
always describe the same division.

**What this changes for a program already written.** With floored as the default, a `div`
with operands of differing signs answers one lower than it used to: `-7 div 3` is `-3`
where it was `-2`. Anything that relied on the old behaviour wants `division =
"truncated"`, which is exactly what `div` used to do. `mod` is unchanged under the
default.

A `.lrc` built before 14 cannot be run by a Luarust that reads 14 — the version check
refuses it outright rather than reading a header that has since grown a field — so there
is no case where the same file quietly answers differently. The change is only visible on
recompiling the source.

## Where the convention lives

`Division` is in `luarust-core`, and `Division::apply` is the one place a quotient and a
remainder are decided together. Everything with a `div` in it takes a `Division`:

- `luarust-conf` reads it from `Luarust.toml`, `luarust-check` from `defaults.division.x;`
- `Checked` and `Chunk` both carry it, and the chunk writes it as a tag
- the JIT passes it as a constant to `cell_binary` and `fallback`, and compiles it into
  `int_division` as one correction tail chosen at compile time — truncated appends
  nothing, floored appends the sign-disagreement correction, euclidean appends its own
- `luarust-num` knows nothing about it. Every float, decimal and rational remainder it
  computes is floored, and `leaning` in `luarust-core` moves that one step to whichever
  convention was asked for. `div` on those types is exact division and has no quotient to
  correct, so there is nothing else to do.

The fuzzers pick a convention from the seed, so a sweep covers all three.

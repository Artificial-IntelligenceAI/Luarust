# The benchmark

`python3 bench/run.py` — builds and runs the same loop in every language of a suite, at
ten million and a hundred million iterations, and prints what each one took.

The loop is `sum = (sum + i) mod 1000000007`. Each value needs the one before it, so it
cannot be folded into a formula, vectorised, or run out of order: everybody actually
loops, and what is being measured is one add and one remainder.

## The standard set

`python3 bench/run.py` runs `std`, which is:

| | |
| --- | --- |
| **C** | `clang -O2` — the floor everything else is a ratio against |
| **Rust** | `rustc -O` — the language this one is written in |
| **Java** | a JIT with thirty years of work in it |
| **JavaScript** | `node`, so V8 — the other JIT with thirty years of work in it |
| **Go** | compiled, garbage collected, and exposes no intrinsics at all |
| **Lua** | the language Luarust is a ripoff of |
| **LuaJIT** | what a very good tracing JIT does with that language |
| **CPython** | the interpreter everybody has actually used |
| **lust-rs** | the closest thing to Luarust that already exists |

Named in `SUITES` in `run.py` rather than being whatever happened to be installed, so a
variant later means naming a different list and not arguing about which rows a table has.
Luarust's own rows are not in the suite: they are what is being measured, and this is the
field.

**lust-rs is in `std` on purpose.** It is Lua-shaped, written in Rust, typed, and it has a
JIT — nearly Luarust's own pitch, arrived at independently. It is the comparison hardest to
explain away, which is the argument for keeping it in the standard set rather than as a
curiosity. `cargo install lust-rs`; its JIT is x86-64 only, so on an arm64 machine the row
is its interpreter. Without it the row says `not installed` rather than going quietly
missing.

**PyPy is known but not standard.** `run.py` can run it and `std` does not ask for it. The
set is a list somebody chose, not everything the machine has.

**Every runner must print the right answer**, worked out here from the closed form rather
than taken from whichever runner went first — reducing at every step and reducing once at
the end give the same result, so the whole loop has one. A timing is not reported for a
run that printed anything else. That is not belt-and-braces: the literal syntax changed
under this file once, and all three Luarust rows spent a while reporting five
milliseconds, which is what three compiler errors take to print.

## How it is measured

Whole-process wall clock, best of three. That counts a JVM starting, V8 warming, and a
Luarust chunk being compiled, because a program that runs is a process that starts. The
slope between the two sizes takes those fixed costs back out and leaves what one iteration
costs.

JavaScript uses plain numbers rather than `BigInt`: every value the loop holds stays under
2^53, so a double holds each one exactly. `BigInt` would be measuring an allocator and is
not what anybody writes for arithmetic this size.

Build the release binary first, with the JIT in it, and the runtime archive beside it, or
the Luarust rows have nothing to measure and the native one cannot be linked:

    LLVM_SYS_211_PREFIX=/opt/homebrew/opt/llvm@21 \
        cargo build --release --all-features -p luarust-cli -p luarust-native

The native row is compiled once and then timed as the program it has become. Building it is
deliberately outside the measurement: that cost belongs to whoever ships the program, not
to whoever runs it, and it is the whole difference between that row and the other Luarust
ones. It needs `cc`; if it cannot be built the row says so rather than quietly going
missing.

Paths to the other runners are named in full at the top of `run.py`, deliberately. The
`java` on this Mac is a stub that resolves to nothing useful and `python3` is whatever came
with the system, so finding them on `PATH` would measure the wrong things or nothing.

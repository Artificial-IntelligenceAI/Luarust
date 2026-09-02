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
| **lust-rs** | typed Lua that compiles, written in Rust |
| **Luau** | typed Lua that compiles, written in C++ |
| **Ravi** | typed Lua that compiles, written in C, through LLVM |

Named in `SUITES` in `run.py` rather than being whatever happened to be installed, so a
variant later means naming a different list and not arguing about which rows a table has.
Luarust's own rows are not in the suite: they are what is being measured, and this is the
field.

## The last three rows are the ones that matter

Everything above them is a language with different aims. These three share the premise:
**take Lua, add static types, and use the types to compile.** That is this project's whole
idea, and all three of them had it first.

- **[Ravi](https://github.com/dibyendumajumdar/ravi)** is the closest by design — a dialect
  of Lua 5.3 with optional static typing, an **LLVM JIT** and an ahead-of-time compiler,
  which is Luarust's architecture down to the code generator. It modifies the VM to exploit
  the types rather than checking them and lowering to stock Lua, which is the line Typed Lua
  and Teal stop at. Written in C. Built from source, against LLVM; there is no package.
- **[Luau](https://luau.org)** is the most finished — Roblox's Lua, gradual types, its own
  native code generator, and more hours of production use than everything else here put
  together. Written in C++. The standalone `luau` CLI is what gets timed, not the engine
  inside Studio.
- **lust-rs** is the closest in implementation — Lua-shaped, typed, JIT, written in Rust.
  `cargo install lust-rs`; its JIT is x86-64 only, so on an arm64 machine the row is its
  interpreter, and `.github/workflows/lust-probe.yml` looks at it on hardware that suits it.

**None of them is "the closest thing to Luarust", and this file said lust-rs was.** That
was one example wearing a superlative: it was the one this project had happened to
investigate, and no comparison had been made. Ravi is closer by design and Luau is closer
by maturity, and which of the three is *closest* depends on the axis, so the table names
the axis instead of ranking them.

A row for something not installed says `not installed` rather than going quietly missing —
the same rule the native row has had since it could fail to link. Three of eleven are
absent on the machine this was written on.

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

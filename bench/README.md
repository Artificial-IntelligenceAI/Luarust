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
| **PyPy** | and what a tracing JIT does with that language |
| **Luau** | typed Lua that compiles, written in C++ |

and every way this language has of running a program:

| | |
| --- | --- |
| **Luarust, native** | compiled ahead of time; the target needs nothing |
| **Luarust, whole JIT** | all of it through LLVM before it starts |
| **Luarust, hot JIT** | interpreted until a loop proves itself |
| **Luarust, bytecode VM** | the bytecode, interpreted |
| **Luarust, tree-walker** | the reference implementation, and the oracle |

**Every method, not the flattering ones.** The tree-walker is in the standard set and it
is an order of magnitude off the VM. A standard set that quietly dropped its slowest row
would be a worse instrument than not having one.

Named in `SUITES` in `run.py` rather than being whatever happened to be installed, so a
variant later means naming a different list and not arguing about which rows a table has —
and so that a row missing from a table is a decision somebody made rather than a tool
somebody forgot to install.

## The last three rows are the ones that matter

Everything above them is a language with different aims. These three share the premise:
**take Lua, add static types, and use the types to compile.** That is this project's whole
idea, and all three of them had it first.

- **[Luau](https://luau.org)** is the most finished — Roblox's Lua, gradual types, its own
  native code generator, and more hours of production use than everything else here put
  together. Written in C++. The standalone `luau` CLI is what gets timed, not the engine
  inside Studio.

**Neither is "the closest thing to Luarust", and this file said lust-rs was.** That was one
example wearing a superlative: it was the one this project had happened to investigate, and
no comparison had been made. The table names the axis each is closest on and ranks nothing.

## Known, and not standard

`KNOWN` in `run.py` is everything the harness can run. Two rows are in it and not in `std`,
because a standard set is a list somebody chose rather than everything that would go.

**lust-rs** — Lua-shaped, typed, JIT, written in Rust, which made it the closest match by
implementation. `cargo install lust-rs`, and its JIT is x86-64 only so an arm64 machine
times its interpreter. Out of `std` on 2026-09-02.

**[Ravi](https://github.com/dibyendumajumdar/ravi)** — and this one is out for a reason
worth writing down, because by design it is the closest thing to Luarust that exists: a
dialect of Lua 5.3 with optional static typing, an **LLVM JIT** and an ahead-of-time
compiler, which is this language's architecture down to the code generator. It modifies the
VM to exploit the types rather than checking them and lowering to stock Lua, where Typed Lua
and Teal stop.

It is out of `std` because installing it is not one command. There is no package; it builds
from source against LLVM, its own Dockerfile pins CMake 3.14, and it has not been touched
since February 2025 — CMake 4 dropped compatibility below 3.10 and all three of its
`CMakeLists.txt` ask for 3.12. And its build defaults `ASAN` to **ON**, so a stock
`cmake .. && make` produces a Ravi crippled by the address sanitizer. A number from that
build sitting in a table next to Luarust's would flatter this language for no reason at
all, which is a worse outcome than an empty row.

`bench/loop.ravi` and its runner are kept for whoever does it properly:
`cmake -DCMAKE_BUILD_TYPE=Release -DASAN=OFF ..`.

A row for something not installed says `not installed` rather than going quietly missing —
the same rule the native row has had since it could fail to link.

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

**Luau runs without `--codegen`, and that is Luau at its best here.** Its native code
generator was measured and is *slower* on this loop — 4.73 ns an iteration against 4.36.
Luau has no integer type, so `%` on numbers is `fmod`, and native code cannot call `fmod`
better than the interpreter can. Wrapping the loop in a function, which is how Luau's
codegen is usually given something to work on, changes nothing. Every other row is its
language at its best and this one is too; the flag is named here so that reads as a
measurement rather than an oversight.

**Which is the asymmetry worth stating, and it matters for exactly one row.** This loop is
a remainder, and a language with 64-bit integers does an integer remainder while a language
whose only number is a double calls `fmod`. Lua 5.4 and later, Java, Go, C, Rust and
Luarust are in the first group; JavaScript, Luau and LuaJIT are in the second.

An earlier version of this file said that was "most of what separates the two groups". It
is not. Fifty million iterations, with and without the remainder, measured rather than
reasoned about:

| | add only | add + remainder | what the remainder costs |
| --- | ---: | ---: | ---: |
| Lua 5.5 | 1.72 ns | 3.81 ns | +2.09 |
| Luau | 2.63 ns | 4.24 ns | +1.61 |
| **LuaJIT** | **0.56 ns** | 4.28 ns | **+3.72** |

**LuaJIT is the row it explains, and it explains the whole of it.** On the bare add it is
0.56 ns — three times faster than Lua 5.5, plainly tracing and compiling the loop, and it
would sit near the top of the table. One `fmod` an iteration costs it almost twice what the
same remainder costs Lua and erases the entire lead. Its 1.91x here is this benchmark being
the worst possible shape for LuaJIT, not LuaJIT being slow, and a table that does not say so
is misleading about the fastest interpreter in the set.

**It does not explain Luau at all.** Luau is slower than Lua 5.5 with the remainder removed
entirely, 2.63 against 1.72, so `fmod` is not why it is behind. Its interpreter is simply
slower than Lua 5.5's on a tight scalar loop. Why is not established here — plausibly it is
tuned for what Roblox actually runs, which is tables and vectors and method calls rather
than arithmetic in a loop — and that is a guess, labelled as one, not a finding.

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

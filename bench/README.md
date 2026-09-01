# The benchmark

`python3 bench/run.py` — builds and runs the same loop in every language here, at ten
million and a hundred million iterations, and prints the three tables the README carries.

The loop is `sum = (sum + i) mod 1000000007`, written seven times over: C, Rust, Java,
Go, Lua, Python and Luarust. Each value needs the one before it, so it
cannot be folded into a formula, vectorised, or run out of order: everybody actually
loops, and what is being measured is one add and one remainder.

**Every runner must print the right answer**, worked out here from the closed form rather
than taken from whichever runner went first — reducing at every step and reducing once at
the end give the same result, so the whole loop has one. A timing is not reported for a
run that printed anything else. That is not belt-and-braces: the literal syntax changed
under this file once, and all three Luarust rows spent a while reporting five
milliseconds, which is what three compiler errors take to print.

Whole-process wall clock, best of three. That counts a JVM starting and a Luarust chunk
being compiled, because a program that runs is a process that starts. The slope between
the two sizes takes those fixed costs back out and leaves what one iteration costs.

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

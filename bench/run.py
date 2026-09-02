#!/usr/bin/env python3
"""Run the same loop in every language here and print what each one took.

The loop is a dependent chain -- `sum = (sum + i) mod 1000000007`, a hundred million
times -- so each iteration needs the previous answer and nothing vectorises. What is
measured is one add and one remainder, repeated, which is the narrowest thing a benchmark
can measure and the least flattering to a compiler that is good at everything else.

Whole-process wall clock, best of three. That counts a JVM starting and a Luarust chunk
being compiled, because a program that runs is a process that starts. Best-of-three rather
than a mean, since the slow runs are the machine's noise and not the language's.

Every path here is explicit. `java` on this Mac is a stub that resolves to nothing useful,
and `python3` is whatever came with the system -- naming the real ones is not fussiness.
"""
import pathlib, shutil, subprocess, sys, tempfile, time

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
RUNS = 3
SIZES = [10_000_000, 100_000_000]

def sized(name, n):
    """One source file with the iteration count filled in."""
    text = (HERE / name).read_text()
    return (text.replace("ITERATIONS_PLUS_ONE", str(n + 1))
                .replace("ITERATIONS_ULL", f"{n}ULL")
                .replace("ITERATIONS_u64", f"{n}u64")
                .replace("ITERATIONS_L", f"{n}L")
                .replace("ITERATIONS", str(n)))

TOOLS = {
    "clang":   "/usr/bin/clang",
    "rustc":   shutil.which("rustc") or "rustc",
    "java":    "/opt/homebrew/opt/openjdk@21/bin/java",
    "javac":   "/opt/homebrew/opt/openjdk@21/bin/javac",
    "lua":     "/opt/homebrew/bin/lua",
    "luajit":  "/opt/homebrew/bin/luajit",
    "pypy3":   "/opt/homebrew/bin/pypy3",
    "python":  "/opt/homebrew/bin/python3.14",
    "go":      "/opt/homebrew/bin/go",
    "node":    shutil.which("node") or "node",
    # `cargo install lust-rs`. Its JIT is x86-64 only, so on an arm64 machine this row is
    # its interpreter and says so; `.github/workflows/lust-probe.yml` is where it gets
    # looked at on hardware that suits it.
    "lust":    shutil.which("lust") or "lust",
    "luarust": str(ROOT / "target/release/luarust"),
}

# The standard set: what Luarust is measured against, and nothing else. Named rather than
# implied, so that adding a variant later means naming a different list here instead of
# arguing about which rows a table happens to have.
#
# Luarust's own rows are not in it. They are what is being measured; this is the field.
SUITES = {
    "std": [
        "C, clang -O2",
        "Rust, rustc -O",
        "Java 21",
        "JavaScript, node",
        "Go 1.26",
        "Lua 5.5",
        "LuaJIT 2.1",
        "CPython 3.14",
        "lust-rs",
    ],
}

# Every language the harness knows how to run, standard set or not. A row here and not in
# a suite is one somebody has to ask for -- PyPy is the first of those, kept because the
# work of running it is already done and dropped from `std` because `std` is a list
# somebody chose rather than everything that happened to be installed.
KNOWN = SUITES["std"] + ["PyPy 7.3"]

def version(tool, *args):
    try:
        out = subprocess.run([TOOLS[tool], *args], capture_output=True, text=True, timeout=30)
        return (out.stdout + out.stderr).strip().splitlines()[0]
    except Exception as why:
        return f"(not run: {why})"

def timed(argv, cwd=None):
    """Best of RUNS, and the answer, so a wrong one cannot post a good time."""
    best, answer = None, None
    for _ in range(RUNS):
        start = time.perf_counter()
        done = subprocess.run(argv, capture_output=True, text=True, cwd=cwd)
        spent = time.perf_counter() - start
        if done.returncode != 0:
            return None, f"exit {done.returncode}: {done.stderr.strip()[:200]}"
        answer = done.stdout.strip()
        best = spent if best is None else min(best, spent)
    return best * 1000, answer

def expected(n):
    """What the loop must print, worked out rather than observed.

    Reducing at every step and reducing once at the end give the same answer, because
    addition modulo a number does not care when the reducing happens. So the whole loop
    has a closed form -- and a runner that quietly optimised the loop away would still
    have to produce this, while one that got the arithmetic wrong could not.
    """
    return str((n * (n + 1) // 2) % 1000000007)


def measure(n, wanted):
    """Every runner, at one size. Returns {name: (ms, answer)}."""
    build = tempfile.mkdtemp(prefix="luarust-bench-")
    for name, out in [("loop.c", "loop.c"), ("loop.rs", "loop.rs"), ("Loop.java", "Loop.java")]:
        pathlib.Path(build, out).write_text(sized(name, n))
    subprocess.run([TOOLS["clang"], "-O2", "-o", f"{build}/loop_c", f"{build}/loop.c"], check=True)
    subprocess.run([TOOLS["rustc"], "-O", "-o", f"{build}/loop_rs", f"{build}/loop.rs"], check=True)
    subprocess.run([TOOLS["javac"], "-d", build, f"{build}/Loop.java"], check=True)
    # Go wants a module around it, and builds into the same folder as everything else.
    pathlib.Path(build, "loop.go").write_text(sized("loop.go", n))
    pathlib.Path(build, "go.mod").write_text("module bench\n\ngo 1.21\n")
    subprocess.run([TOOLS["go"], "build", "-o", f"{build}/loop_go", "loop.go"], cwd=build, check=True)
    for name in ("loop.lua", "loop.py", "loop.js", "loop.lust"):
        pathlib.Path(build, name).write_text(sized(name, n))

    took = {}

    def row(label, argv, cwd=None):
        """Run one language, if this suite asked for it and the machine has it."""
        if label not in wanted:
            return
        if not pathlib.Path(argv[0]).exists() and shutil.which(argv[0]) is None:
            # Said out loud rather than left as a missing row, the same as the native one.
            took[label] = (None, f"not installed: {argv[0]}")
            return
        took[label] = timed(argv, cwd)

    row("C, clang -O2", [f"{build}/loop_c"])
    row("Rust, rustc -O", [f"{build}/loop_rs"])
    row("Java 21", [TOOLS["java"], "-cp", build, "Loop"])
    row("PyPy 7.3", [TOOLS["pypy3"], f"{build}/loop.py"])
    row("Lua 5.5", [TOOLS["lua"], f"{build}/loop.lua"])
    row("LuaJIT 2.1", [TOOLS["luajit"], f"{build}/loop.lua"])
    row("Go 1.26", [f"{build}/loop_go"])
    row("JavaScript, node", [TOOLS["node"], f"{build}/loop.js"])
    row("lust-rs", [TOOLS["lust"], f"{build}/loop.lust"])

    # Luarust's engines. The project file goes beside a copy of the source, so the one in
    # the repository is never rewritten to run a benchmark.
    for label, mode, command in [
        ("Luarust, whole JIT", "whole", "run"),
        ("Luarust, hot JIT", "hot", "run"),
        ("Luarust, bytecode VM", "vm", "run"),
        ("Luarust, tree-walker", "vm", "interp"),
    ]:
        folder = tempfile.mkdtemp(prefix="luarust-bench-lr-")
        pathlib.Path(folder, "loop.lr").write_text(sized("loop.lr", n))
        pathlib.Path(folder, "Luarust.toml").write_text(f'[run]\nmode = "{mode}"\n')
        took[label] = timed([TOOLS["luarust"], command, f"{folder}/loop.lr"])

    # Ahead of time: compiled once here, then timed as the program it now is. The build
    # is deliberately outside the measurement -- that cost is paid by whoever ships it,
    # not by whoever runs it, which is the whole difference between this row and the rest
    # of the Luarust ones.
    folder = tempfile.mkdtemp(prefix="luarust-bench-native-")
    pathlib.Path(folder, "loop.lr").write_text(sized("loop.lr", n))
    pathlib.Path(folder, "Luarust.toml").write_text('[run]\nmode = "vm"\n')
    built = subprocess.run(
        [TOOLS["luarust"], "native", f"{folder}/loop.lr"], capture_output=True, text=True
    )
    if built.returncode == 0:
        took["Luarust, native"] = timed([f"{folder}/loop"])
    else:
        # Said out loud rather than left as a missing row. It needs `cc` and the runtime
        # archive, and a table quietly one row short is worse than one that explains.
        took["Luarust, native"] = (None, f"not built: {built.stderr.strip()[:120]}")

    row("CPython 3.14", [TOOLS["python"], f"{build}/loop.py"])
    return took


def main():
    asked = sys.argv[1] if len(sys.argv) > 1 else "std"
    if asked not in SUITES:
        print(f"no suite named {asked!r}. There is: {', '.join(sorted(SUITES))}")
        return 2
    wanted = SUITES[asked]
    print(f"suite: {asked} — {', '.join(wanted)}")
    at = {n: measure(n, wanted) for n in SIZES}
    small, big = SIZES[0], SIZES[-1]

    wrong = [
        (n, name, answer)
        for n, took in at.items()
        for name, (ms, answer) in took.items()
        if ms is not None and answer != expected(n)
    ]
    missing = {name for took in at.values() for name, (ms, _) in took.items() if ms is None}
    for name in sorted(missing):
        print(f"  {name}: {at[big][name][1]}")
    if wrong:
        for n, name, answer in wrong:
            print(f"  WRONG at {n}: {name} said {answer}, not {expected(n)}")
        return 1

    print(f"\nsum = (sum + i) mod 1000000007, best of {RUNS}, whole-process wall clock\n")
    base = at[big]["C, clang -O2"][0]
    print(f"| | {big // 1_000_000}M | vs C |")
    print("| --- | --- | --- |")
    for name, (ms, _) in sorted(at[big].items(), key=lambda r: r[1][0] or 1e9):
        if ms is None:
            continue
        print(f"| {name} | {ms:,.0f} ms | {ms / base:.2f}x |")

    # The slope between the two sizes drops whatever a runner spends before it loops -- a
    # process launching, a JVM warming, LLVM compiling -- and leaves the iteration itself.
    print(f"\n| | ns/iter | vs C | {small // 1_000_000}M | {big // 1_000_000}M | ratio |")
    print("| --- | --- | --- | --- | --- | --- |")
    per = {
        name: (at[big][name][0] - at[small][name][0]) * 1e6 / (big - small)
        for name in at[big]
        if name not in missing
    }
    for name, ns in sorted(per.items(), key=lambda r: r[1]):
        lo, hi = at[small][name][0], at[big][name][0]
        print(f"| {name} | {ns:.2f} | {ns / per['C, clang -O2']:.2f}x | {lo:,.0f} ms | {hi:,.0f} ms | {hi / lo:.1f}x |")
    print(f"\n  every runner printed {expected(big)} at {big // 1_000_000}M and "
          f"{expected(small)} at {small // 1_000_000}M, which is what the closed form says.")
    print("  a ratio near 1x would mean a loop was replaced by a formula; ten times the")
    print("  work should take about ten times the time, less whatever was not the loop.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

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
    "luarust": str(ROOT / "target/release/luarust"),
}

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


def measure(n):
    """Every runner, at one size. Returns {name: (ms, answer)}."""
    build = tempfile.mkdtemp(prefix="luarust-bench-")
    for name, out in [("loop.c", "loop.c"), ("loop.rs", "loop.rs"), ("Loop.java", "Loop.java")]:
        pathlib.Path(build, out).write_text(sized(name, n))
    subprocess.run([TOOLS["clang"], "-O2", "-o", f"{build}/loop_c", f"{build}/loop.c"], check=True)
    subprocess.run([TOOLS["rustc"], "-O", "-o", f"{build}/loop_rs", f"{build}/loop.rs"], check=True)
    subprocess.run([TOOLS["javac"], "-d", build, f"{build}/Loop.java"], check=True)
    for name in ("loop.lua", "loop.py"):
        pathlib.Path(build, name).write_text(sized(name, n))

    took = {}
    took["C, clang -O2"] = timed([f"{build}/loop_c"])
    took["Rust, rustc -O"] = timed([f"{build}/loop_rs"])
    took["Java 21"] = timed([TOOLS["java"], "-cp", build, "Loop"])
    took["PyPy 7.3"] = timed([TOOLS["pypy3"], f"{build}/loop.py"])
    took["Lua 5.5"] = timed([TOOLS["lua"], f"{build}/loop.lua"])
    took["LuaJIT 2.1"] = timed([TOOLS["luajit"], f"{build}/loop.lua"])

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

    took["CPython 3.14"] = timed([TOOLS["python"], f"{build}/loop.py"])
    return took


def main():
    at = {n: measure(n) for n in SIZES}
    small, big = SIZES[0], SIZES[-1]

    wrong = [
        (n, name, answer)
        for n, took in at.items()
        for name, (_, answer) in took.items()
        if answer != expected(n)
    ]
    if wrong:
        for n, name, answer in wrong:
            print(f"  WRONG at {n}: {name} said {answer}, not {expected(n)}")
        return 1

    print(f"\nsum = (sum + i) mod 1000000007, best of {RUNS}, whole-process wall clock\n")
    base = at[big]["C, clang -O2"][0]
    print(f"| | {big // 1_000_000}M | vs C |")
    print("| --- | --- | --- |")
    for name, (ms, _) in sorted(at[big].items(), key=lambda r: r[1][0]):
        print(f"| {name} | {ms:,.0f} ms | {ms / base:.2f}x |")

    # The slope between the two sizes drops whatever a runner spends before it loops -- a
    # process launching, a JVM warming, LLVM compiling -- and leaves the iteration itself.
    print(f"\n| | ns/iter | vs C | {small // 1_000_000}M | {big // 1_000_000}M | ratio |")
    print("| --- | --- | --- | --- | --- | --- |")
    per = {name: (at[big][name][0] - at[small][name][0]) * 1e6 / (big - small) for name in at[big]}
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

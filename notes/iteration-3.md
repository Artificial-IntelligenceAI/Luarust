# Iteration 3

Kept here rather than in a chat, because it has been lost to a context compaction twice.
Fifteen crates, about 25,500 lines of Rust as this is written.

**`[run] chunks` is settled** — see `notes/trusted-chunks-what-it-may-skip.md`. It stays,
it travels in the chunk like `[run] mode`, and the rule is that it may skip a check whose
failure is a wrong answer and never one whose failure is a read outside an allocation. The
key name is still unnamed.

## Done

**The hot JIT** — `[run] mode = "hot"`. Interprets, counts back edges, and when a loop
passes ten thousand compiles what that loop can reach and enters it at the loop head with
the VM's live registers. Takes loops in routines as well as at the top level, keeps the
machine code for a routine that goes hot so later calls land on it, and enters a kept call
in 85 ns rather than 160. `notes/the-hot-jit-enters-mid-loop.md` has the whole shape.

**Ahead-of-time native output** — `luarust native file.lr`. LLVM writes an object, `cc`
links it against `luarust-native`, and what comes out runs on a machine with nothing
installed: no LLVM, no `luarust`, no chunk. 1.6 MB stripped against the toolchain's 56 MB.

The thing that made it work is a crate split. The runtime used to live inside
`luarust-jit`, so linking against it would have dragged thirty-two megabytes of LLVM into
every shipped program. It is `luarust-runtime` now, with no compiler in it at all, and
`luarust-jit` depends on it rather than owning it. LLVM belongs to the machine that
*builds* a program.

**Array loops are level with C, and were ninety times off it until 2026-09-01.** Summing a
hundred thousand `ui64` elements repeatedly, slope taken between 300 and 3000 rounds so
process start is out of it:

                        before      after
    C, clang -O2        0.06 ns     0.05 ns
    Luarust, native     5.18 ns     0.05 ns

**What was wrong.** A loop over an array made two runtime calls on every element --
`luarust_array_base` to find where the elements are, `luarust_array_len` to check the
index. Both were declared with no attributes, so LLVM had to assume each might write any
memory. Nothing could be hoisted across them and nothing vectorised, and the storage
underneath was fine the whole time: an array of `ui64` really is a run of 64-bit words and
the element read really was an ordinary load.

**What took a whole evening.** Saying `readonly` on both is true, lands in the IR, and
changes nothing. Running the pipeline twice changes nothing. Setting the loop-vectorisation
and unrolling tuning options changes nothing. Setting the module triple changes nothing.
And all the while `opt -passes='default<O3>'` on the *same* module hoisted the call clear
out to the outermost preheader, and `opt -passes='loop-mssa(licm)'` alone did it too.

The difference was the spelling. `readonly` is the old name for the attribute; LLVM
upgrades one when it *parses* it, which is why `opt` acted on it and setting the enum
attribute in-process did not. Nothing modern looks at the legacy name. Setting `memory`
instead -- the bitmask, two bits a location, `Ref` in every one -- hoists the call and
vectorises the loop.

Worth knowing for the next time an attribute seems to do nothing: check what the *printed*
IR calls it, not what you set.

**And the VM was paying for errors it never had.** `Op::At` read an element with
`ok_or(Stopped { fault: out_of_range(...) })`. `ok_or` builds its argument whether it is
wanted or not, so every element that *was* there still paid to describe the one time it
might not be: `out_of_range` formats a message, formatting allocates a `String`, and it
asked the heap for the array's length again in order to do it. A profile of the loop had
thirty per cent of its samples inside that, building faults for a program that had none.

    array loop, 30M reads      before      after
      bytecode VM              3,385 ms    478 ms
      tree-walker              1,010 ms  1,013 ms

Three things, in the order a profile found them. `ok_or` built a fault -- formatting a
message, allocating a `String`, asking the heap for the length again -- for every element
that was fine: `ok_or_else` and it is gone, 3,385 to 651. Then `offset` looked the array's
*shape* up per element, through another thread local and another `RefCell`, to fetch
something that cannot change while a program runs: resolved once when the instruction is
widened, 569 to 478. Between them, dropping a redundant length lookup, 588 to 569.

For scale, on the same loop: LuaJIT 11 ms, Lua 5.5's VM 76 ms, CPython 1,081 ms. Six times
Lua's VM is a great deal better than forty-four times, and is still six times. What is left
is `heap::read` itself -- a thread local, a `RefCell` borrow, a table index and a `Value`
built, for every element -- which is the same machinery the `memory(read)` caveat below
wants replaced by a spans table read through a raw pointer. One piece of work would answer
both.

**And it broke something, which the fuzzer caught.** Dropping the length check from
`offset` was safe for reads, because `heap::read` checks and the error path reports it. It
was not safe for *writes*: `heap::store` also checks and returns whether it wrote, and
`Op::StoreAt` had been discarding that answer for as long as `offset` made it unreachable.
An out-of-range write became a silent no-op. Seed 7894, found in the 200,000 the change was
gated on, fixed by having the write path report what the read path already reported. The tell was there to be read for a long time: the VM was
*three times slower than the tree-walker* on array code, which is backwards, and the reason
is that the tree-walker already wrote `ok_or_else` on the same line.

**One caveat on the claim.** `memory(read)` says these calls only read. They also take a
`RefCell` borrow, which writes a flag -- so it is very slightly stronger than the truth.
Nothing compiled ever reads that flag, every borrow is balanced inside the call, and the
ordering that actually matters still holds: the calls that *grow* an array carry no
attributes, so LLVM treats them as writing everything and will not lift a read across one.
Making the claim exactly true means reading the base and length without touching the
`RefCell` -- a small spans table read through a raw pointer -- and is worth doing.

**What is left in the VM is structural, and three attempts at it made things worse.**
Tried on 2026-09-01, after the wins above:

- Reading number elements as bare bits, skipping the `Value` the general path builds:
  **+77%**, and the regression was on the *addition* loop, which has no array in it.
- The same without the `continue` that seemed the likely culprit: **+77%** again.
- Moving the whole array-read arm out of the dispatch loop behind `#[inline(never)]`:
  **+10%**, which is inside the noise below and so proves nothing either way.

The first two are the finding. `run_with` is one very large function and every arm shares
its registers, so code added to the arm that reads an array made the arm that adds two
numbers three-quarters slower, with nothing about addition changed. Anything that grows
that function pays for it everywhere, which rules out superinstructions and typed
fast-paths as they were sketched, and means the next attempt wants a plan for the whole
loop rather than another arm.

**And a warning about measuring it.** Two builds of *identical* source, compared with
best-of-six interleaved, came out 189 ms and 204 ms — eight per cent apart, from code
layout alone. Anything smaller than about ten per cent cannot be told from that with this
method, so a change of that size needs either many more samples or a way of measuring that
does not rebuild.

For scale, on the same loop: Lua 5.5's VM does what takes this one 14.1 ns in 2.5. That
gap is a computed-goto dispatch over a compact encoding, against a Rust `match` over a
twelve-byte enum — structure, not a missing trick.

## Left

**External FFI — calling C from Luarust.** Not the internal boundary that already exists
(compiled code calling `luarust_print_text` and the rest of `luarust-runtime`). This is the
language gaining a way to declare a foreign function and call it. Four things have to be
decided before any of it can be written, and three of them are design calls:

- **Which types may cross.** The integer widths and `b32`/`b64` map onto C directly.
  `str`, `er`, arrays, the decimals and `b128`/`b256` do not, and each needs a
  representation decision or a refusal.
- **How each path calls out.** Native code can let the linker resolve the symbol. The JIT
  would resolve it at run time. The VM and the tree-walker have no machine code to put a
  call into at all, so they need either a libffi-style dynamic call or a generated thunk —
  and if they cannot call out, a program using FFI stops being runnable three ways.
- **What it does to the oracle.** This is the sharp one. The whole testing architecture
  rests on three implementations computing the same answer, and the generator writing
  programs whose answers are checkable. A foreign call has effects outside Luarust that no
  path can independently compute, so FFI programs cannot be differentially tested the way
  everything else is. Whatever the design, it should say what replaces that guarantee.
- **What it does to the character of the language.** Luarust refuses to guess, checks what
  it can prove, and reports faults rather than crashing. One `extern` call can corrupt any
  of that, and no amount of checking on this side prevents it. Whether the boundary is
  marked in the source, and how loudly, is a design call rather than an implementation one.

**Parallel loops.** Added 2026-09-01. The counted loop is unusually well shaped for it --
`loop.temp.range.ui64 ['i'] = [|1|, |n|]` has known bounds and a counter the body cannot
assign to, which is what every auto-paralleliser wants and what a `while` never gives.

The grammar already does modifiers with dots, so this is another dot rather than a new
construct, and deleting it leaves the sequential loop:

```luarust
loop.temp.parallel.range.ui64 ['i'] = [|1|, |n|] { ... }
```

That property is not cosmetic. It is what keeps the oracle alive: the tree-walker ignores
the modifier and runs the loop in order, and if the independence the user asserted is real,
all three paths still agree. If it is not real, they disagree and the fuzzer says so --
which is the right failure.

Four things stand in the way, in order of difficulty:

- **The heap is one thread's.** `luarust-core::heap` is a `thread_local!` holding every
  array the program has made, and so are the runtime's frames, output and constants.
  Iterations touching arrays on several cores need it shared and locked, or partitioned.
  That is a different runtime, not an adjustment to this one.
- **Arrays alias.** A handle is a four-byte index and `Op::Move` copies the handle rather
  than the array, so two names can mean one array. Proving two iterations do not touch the
  same element is the hard half of dependence analysis.
- **Reassociation changes answers.** Float addition is not associative, and this language
  prints exact values -- a reordered sum does not round differently, it *prints*
  differently. Under `overflow = "trap"` a partial sum can also trap where the sequential
  one never would. OpenMP's answer is to make the programmer name the operator
  (`reduction(+:sum)`); the other answer is to refuse accumulating loops in the first
  version and allow only ones that write to distinct places.
- **Printing inside one** interleaves differently every run. Either forbid it or order it.

The model to copy is the annotation family -- OpenMP, Fortran's `do concurrent`, Julia's
`Threads.@threads` -- and not the library family (Rayon, `Parallel.For`, Java streams),
which needs closures and iterators that Luarust does not have.

**Threads the programmer allocates.** Raised 2026-09-01, not yet a decision. Worth keeping
apart from parallel loops, because they are different problems wearing similar words.

A parallel loop is *structured*: the parallelism starts at the loop and ends at it, and
nothing escapes. That is why it is tractable, and why the answer stays deterministic. A
thread the programmer allocates is *unstructured* -- it can outlive whatever made it, hold
whatever it was given, and run in an order nobody chose. Every hard thing about concurrency
lives on that side of the line.

For this language the sharp cost is the oracle again, and worse than FFI's. A parallel loop
over independent iterations has one right answer, so three implementations can still be
made to agree. Two threads sharing state have *many* right answers, and a test that insists
three paths print the same thing has nothing left to insist on.

There is a third model that dodges most of it, and the existing design has already half
built it: **isolated tasks that share nothing and pass messages**, as Erlang does. The heap
being a `thread_local!` is exactly the wrong shape for parallel loops and exactly the right
shape for this -- a task gets its own heap for free, values are copied across rather than
shared, and no lock is needed anywhere because nothing is reachable from two places. It
does not restore determinism, but it gives memory safety without asking for any of the
machinery shared-memory threads would need.

So the order, if any of it happens: parallel loops first, isolated tasks second, and
shared-memory threads last if ever, because that one argues with the first line of the
README.

## Not iteration 3

**Inline `$bash { }` was pulled off the list on 2026-09-01**, to optimise first. Kept here
rather than deleted, because the reasoning still holds if it comes back:

**Inline `$bash { }`, gated on native output.** Only meaningful for a program that is
becoming a binary, because a chunk that runs anywhere cannot promise a shell exists there.
The syntax sketch is `$<language> { ... }`. Open: what the equivalent is per platform, and
what crosses the boundary in each direction.


Caching aside, the hot JIT still cannot count *bare calls* — a routine with no loop in it,
called a great many times, never trips anything. Measured and deliberately deferred: an
interpreted leaf call costs 38 ns and entering compiled code costs 85, so a policy that
counted calls without a body filter would compile leaves into something slower.
`crates/luarust-jit/tests/call_cost.rs` is the ruler.

# Luarust
Project under development, stable use not recommended, stability is **not a guarantee**

**Luarust** is a **Lua ripoff** focused on performance, device support, explicit syntax, and very helpful error messages (unlike fucking `C`, joking 😂).

Luarust's method is focused on compile once, run anywhere (Like Java's), boosting its performance via JIT compilation, with **LLVM** doing the code generation. The final product only contains what is needed (unlike fucking `Go`, joking, again 😂)

So, to put it simply. Luarust is a computer language that has **very helpful error messages**, and **could run basically anywhere**, **without sacrificing much performance**.

We ran some benchmarks, **Luarust is one of the slowest JIT ever 😭.** 

### Why pick Luarust, and why not
| Why pick Luarust | Why not |
| --- | --- |
| Compile once, run basically anywhere | One of the slowest JIT languages |
| High-level | No direct electricity manipulation |
| Final product only has the things that are needed (once again, unlike fucking `Go` 🤣) | It doesn't give you everything (e.g. time manipulation) |
| Very helpful error messages (hahaha, guess what? Unlike fucking `C` 🤣) | Error messages may be too long |
| | Stability is not guaranteed |

### Why pick Luarust, and why not (no jokes)

| Why pick it | Why not |
| --- | --- |
| An error names the rule that was broken and the fix, points at the line, and apologises for the interruption | That is a lot of polish on a language that cannot yet write a function |
| Nothing is implicit — no conversion, no truthiness, no coercion. Two types meet only where something said they should | Saying so is wordy. `var.local.mut.ui32 ['total'] = [\|0\|];` is a lot of characters for a counter |
| Real IEEE 754, correctly rounded, in five binary formats — including `b128` and `b256`, which almost nothing else on earth implements | The decimal formats and `er` are in the type list and not yet built. A `d64` answers with an error, not a number |
| Compile once, run anywhere is literal: one `.lrc` file, little-endian everywhere, and a **461 KB** runtime to run it on | Building the JIT needs LLVM 21 on the machine that builds it. That is a big dependency for a small language |
| Three implementations — a tree-walker, a bytecode VM, and an LLVM JIT — that must agree bit for bit on 200,000 generated programs before anything ships | Three implementations is also three places for a bug to hide, and the fuzzer has found real ones |
| Only what a program uses gets delivered. No garbage collector, no parser, no JIT on the device | There is not much to leave out yet, so this is a promise about the future as much as a fact about now |
| 1.27× C on the dependent-chain benchmark, from a language nobody has optimised | 1.27× C is still last place behind C, Rust and Java, and the bytecode VM is slower than Lua |
| It is small enough to read. Thirteen crates, about 13,000 lines, and the whole thing fits in a head | It is one person's hobby project at version 0.0.0, and stability is **not a guarantee** |

## Declaring things

A declaration says the scope and the type up front, then takes a list of names and a
list of values. Statements end with a semicolon.

```luarust
var.local.str ['name'] = [|Tankun|];
```

Names live in quotes, so a name can be anything you can type — spaces, punctuation,
emoji, whatever you actually wanted to call it:

```luarust
var.local.str ['a friendly greeting'] = [|hello|];
var.local.b16 ['❔']                  = [|1000|];
```

Written values wear bars instead, so a quoted thing is a name wherever you meet one and
never has to be read as a value depending on where it sits. The type is what decides how
a written value reads — the same four characters are a number under `b16` and text under
`str`:

```luarust
var.local.b16 ['x'] = [|1000|];    -- the number 1000
var.local.str ['y'] = [|1000|];    -- the text "1000"
```

### Several at once

The brackets hold lists, so one declaration can make several variables that share a
scope and a type:

```luarust
var.local.str ['name', 'name 2', 'name 3'] = [|Tankun|, |Ada|, |Jensen Huang|];
```

Whatever they have in common hoists onto `var`, and whatever they don't goes inline.
Same scope, different types:

```luarust
var.local [str 'a', b16 'b'] = [|hello|, |1000|];
```

Nothing in common at all:

```luarust
var [local.str 'name', local.b16 'x', local.str '❔'] = [|Tankun|, |1000|, |idk|];
```

The two lists have to be the same length. Three names and two values is an error, and
it names the one that went without — Luarust will not invent a value you did not
write.

## Changing things

`var` makes a variable. `set` changes one that already exists:

```luarust
var.local.mut.b16 ['x'] = [|1000|];
set ['x'] = [math { 'x' + 1 }];
```

A variable cannot be changed unless it said it could. `mut` goes in the chain, and
without it that second line is an error — one that names the word you left out rather
than the line you wrote it on.

`set` carries no visibility and no type, because the variable already has both. It takes
the same lists as everything else:

```luarust
var.local.mut.b16 ['a', 'b'] = [|1|, |2|];
set ['a', 'b'] = [|10|, |20|];
```

`mut` hoists like the rest of the chain, so names that share it say it once:

```luarust
var.local.mut [b16 'a', b64 'b'] = [|1|, |2|];
```

### Adding to something

Adding to a variable is common enough to be worth its own word. Said with `set`, the
name has to appear twice and the brackets get in the way of a very small idea:

```luarust
set ['total'] = [math { 'total' + 'i' }];
handback 'i' as 'total';                     -- the same thing
```

`handback` adds, and only adds. A running product or a subtraction is a `set` — piling
operations onto `handback` would cost it the readability that is its whole reason for
existing. The target has to be `mut`, the same as any other change.

## Printing

`print` takes its items in brackets, juxtaposed rather than separated by anything:

```luarust
var.local.b16 ['x'] = [|1000|];
print["x is equal to " 'x' \n];
```

Double quotes are text. **Single quotes are a name** — `'x'` is the variable, not the
letter x. That is how a value gets read back out of one.

`\n` is written as a bare token outside the quotes, and nothing is inserted on your
behalf: no separator between items, and no line ending unless you write one.

A number prints as the value that is actually stored, not as the text that was written
to make it. `b16` carries eleven bits of significand, and most decimals are not in it:

```luarust
var.local.b16 ['a'] = [|0.1|];
print['a' \n];                 -- 0.0999755859375
```

That *is* `b16 '0.1'` — the nearest `b16` to a tenth, exactly. Luarust will not print a
tidier number than the one it is holding.

## Arithmetic

Arithmetic happens inside `math { }` and nowhere else. A math block stands where a
value stands:

```luarust
var.local.b16 ['x', 'y'] = [|3|, |4|];
var.local.b16 ['z']      = [math { 'x' + 'y' }];
print['z' \n];                                     -- 7
```

Single quotes mean here what they mean in a print list: `'x'` is the variable. Numbers,
though, are written bare:

```luarust
var.local.b16 ['z'] = [math { 'x' + 1 }];
```

Bars exist so that a type annotation can decide what a written value means — `|1000|` is a
number under `b16` and text under `str`. A bare number takes its type from its
surroundings and needs no bars at all, so inside a math block you rarely see them.

Where nothing supplies a type, a value can say what it is:

```luarust
var.local.bool ['yes'] = [math { ui32 |12| < ui32 |13| }];
```

A comparison tells its two sides nothing about themselves — that is the point of one — so
without this there would be nowhere for `12` to get a type from. It is still a literal and
still checked: `ui8 |300|` is out of range, and a stated type may not disagree with one
already expected.

Grouping is `( )`, because `[ ]` and `{ }` are both spoken for:

```luarust
var.local.b16 ['w'] = [math { ('x' + 'y') * 'x' }];   -- 21
```

Most operators have more than one spelling, and they all mean exactly the same thing:

| | | |
| --- | --- | --- |
| add | `+` | |
| subtract | `-` | |
| multiply | `*` | `x` |
| raise to a power | `**` | `xx` or `pow` |
| divide | `/` | `÷` or `div` |
| remainder | `mod` | |
| less than | `<` | |
| greater than | `>` | |
| less than or equal | `</=` | `<=` or `≤` |
| greater than or equal | `>/=` | `>=` or `≥` |
| equal to | `=` | |
| not equal to | `!=` | `not=` or `≠` |

`%` is **not** remainder. Mathematics has never used it for one — it is percent, written
after a number, so `15%` is fifteen hundredths:

```luarust
var.local.d64 ['price'] = [|19.99|];
var.local.d64 ['vat']   = [math { 'price' x 20% }];
```

Every number in Luarust is a float, so a percentage is an ordinary value rather than an
awkward one, and in a decimal type `20%` is exactly a fifth.

Remainder is the operation mathematics and the C family disagree about, and Luarust sides
with mathematics: the result takes the sign of the **divisor**, not the dividend.

```luarust
math { -7 mod 3 }     -- 2,  not -1
math { 7 mod -3 }     -- -2, not 1
```

So `'i' mod 3` cycles `0 1 2` however `'i'` is signed, which is what anyone counting
actually wanted.

### Comparing

Six of them, and they answer `bool`:

```luarust
var.local.i32 ['a', 'b'] = [|3|, |5|];

var.local.bool ['less']    = [math { 'a' <   'b' }];    -- true
var.local.bool ['more']    = [math { 'a' >   'b' }];    -- false
var.local.bool ['at most'] = [math { 'a' </= 'a' }];    -- true
var.local.bool ['same']    = [math { 'a' =   'a' }];    -- true
var.local.bool ['differs'] = [math { 'a' !=  'b' }];    -- true
```

`</=` and `>/=` are one operator each, and the `/` in them is the "or" of *less than or
equal* — not a division. Nothing else could ever follow a `<` with a `/`, so there is
nothing for it to be confused with.

`=` here is not the `=` of a declaration, and cannot be mistaken for it: that one only ever
sits between a list of names and a list of values, and this one only ever sits inside a
math block.

A comparison is the loosest thing in a math block, so `math { 'a' + 1 < 'b' }` compares the
sum, which is how it reads.

**A comparison is not chained.** Mathematics reads `a < b < c` as both comparisons at once
and most languages read it as comparing a `bool` to a number, which is nothing at all.
Luarust refuses to pick:

```
error[E0114]: there are two comparisons here.
```

**A NaN answers false to all of them but one.** It is not less than, greater than, or equal
to anything, itself included — so `math { 'n' = 'n' }` is `false` for a NaN, and so is
`</=`. The exception is `!=`, which asks only that the two differ, and a NaN differs from
everything:

```
nan <   nan     false
nan </= nan     false
nan =   nan     false
nan !=  nan     true
```

`<`, `>`, `</=` and `>/=` put numbers in order. `=` and `!=` work on anything, since two
things of the same type are either the same or they are not.

Words work as operators here because a name is always quoted. `'x'` is a variable and a
bare `x` cannot be one, so there is nothing for `math { 'a' x 'b' }` to be confused with.

### Joining conditions

`and`, `or` and `not`, as words. They take `bool` and they answer `bool`, and there is no
truthiness anywhere: a number is not a question and Luarust will say so.

```luarust
var.local.bool ['in range'] = [math { 'n' > i32 |0| and 'n' < i32 |100| }];
var.local.bool ['outside']  = [math { 'n' < i32 |0| or  'n' > i32 |99|  }];
var.local.bool ['missing']  = [math { not 'in range' }];
```

`or` is looser than `and`, which is looser than `not`, which is looser than a comparison.
So `'a' > 'b' and 'c' > 'd'` groups the way it reads, and `not 'a' = 'b'` asks whether
they are *not* equal rather than turning one side around.

**The right side is not worked out when the left already settled it.** That is not an
optimisation you are being told about for interest — it is what lets a condition guard
the one after it:

```luarust
if [math { 'd' != i32 |0| and 'n' div 'd' > i32 |1| }] { … }
```

With `d` at zero the division never happens, so the program does not stop. If both sides
were always worked out there would be no way to write that at all.

That is six kinds of bracket, and the reason it works is that no two of them ever
mean the same thing:

| | |
| --- | --- |
| `[ ]` | a list — of names, of values, of things to print, or a condition |
| `{ }` | a block — the word in front of it says which kind |
| `( )` | grouping, inside a math block |
| `' '` | a name — always |
| `\| \|` | a written value — always |
| `" "` | text, in a print list |

You never have to work out which sense a bracket is being used in. It only has one.

## Deciding

```luarust
var.local.i32 ['n'] = [|12|];

if [math { 'n' > i32 |10| }] {
    print["big" \n];
} else-if [math { 'n' = i32 |10| }] {
    print["exactly ten" \n];
} else {
    print["small" \n];
}
```

The condition goes in `[ ]`, like every other construct's arguments, and the body in
`{ }`, like every other block. `else-if` is one word — the same hyphen that joins
`no-visibility-stated` joins this.

Arms are asked in order and exactly one body runs. A condition after the one that held is
never reached, let alone asked. The `else` is optional, comes last, and there is only ever
one of it; anything else is an error that says so.

An `if` has no `temp`/`perm` in its chain, and that is not an oversight. A loop needs one
because a loop *introduces a counter* and somebody has to say whether it outlives the
block. An `if` introduces nothing, so a variable declared inside an arm is simply gone at
the closing brace.

A condition may be a variable on its own, since a name in quotes is a name everywhere:

```luarust
if ['flag'] { … }
```

## Loops

A loop is written the way a declaration is, because it does the same thing a declaration
does: a chain saying how long the counter lives, what kind of loop it is, and what type
the counter has — then a name, then the values that set it going.

```luarust
loop.temp.range.ui8 ['i'] = [|1|, |5|] {
    print['i' \n];
}
```

```
1
2
3
4
5
```

`loop.range` is what makes those two values bounds rather than two initialisers, and the
bounds are **inclusive**: one to five is five passes, the way a range is read in
mathematics.

### How long the counter lives

`temp` and `perm` say whether the counter outlives the loop, and one of them has to be
said:

```luarust
loop.temp.range.ui8 ['i'] = [|1|, |5|] { … }    -- 'i' is gone at the brace
loop.perm.range.ui8 ['i'] = [|1|, |5|] { … }    -- 'i' is still there afterwards
```

A `perm` counter holds **the last value it actually took** — five, not six. Languages
that leak their counter usually leave it one past the end, which is a side effect of how
their loops are built rather than anything a person asked for.

Most languages decide this for you and expect you to know which. Python leaks the
counter, Rust and Lua scope it away, and neither says so anywhere in the loop. Here it is
a word you type.

### Something to accumulate into

A counter belongs to its loop either way, so anything that has to survive is declared
before it:

```luarust
var.local.mut.ui32 ['total'] = [|0|];

loop.temp.range.ui32 ['i'] = [|1|, |10|] {
    handback 'i' as 'total';
}

print["total is " 'total' \n];
```

```
total is 55
```

`total` needs `mut` because the loop changes it. And the counter is `ui32` for the same
reason everything else in Luarust matches exactly: nothing converts on its own, so a
`ui8` counter could not be added to a `ui32` total — not because it would not fit, but
because they are two different types and the language will not quietly bridge them.

Notice there is **no semicolon after the closing brace**. A semicolon ends a statement
that finishes on a value, and a block already says where it ends. `};` would be saying
it twice.

## Timing

`time.now` is a value: how many seconds the clock has counted, read as whatever float
type is asked of it. Take it twice and subtract to find out how long something took.

```luarust
var.local.mut.ui64 ['sum'] = [|0|];
var.local.b64 ['start']    = [time.now];

loop.temp.range.ui64 ['i'] = [|1|, |100000000|] {
    set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];
}

var.local.b64 ['elapsed'] = [math { time.now - 'start' }];

print['sum' " in " 'elapsed' " seconds\n"];
```

`time.now` is unquoted, so it cannot be mistaken for a variable — a name is always in
quotes — which is what lets it sit inside a math block next to one.

The clock is **monotonic**: it only ever moves forward, and it does not know what time
of day it is. A wall clock can step backwards when the machine corrects itself against a
time server, and a benchmark that reports a negative duration for that reason is worse
than one that reports nothing. What is lost is the ability to ask what time it is, which
is not what a timer is for.

## Scope

Every declaration carries one of four, and the first three mean what they usually mean:

```luarust
var.local.str      ['name'] = [|Tankun|];   -- the block it is written in, and no further
var.global.str     ['name'] = [|Tankun|];   -- the whole program
var.public.str     ['name'] = [|Tankun|];   -- and exported, so importers see it too
var.restricted.str ['name'] = [|Tankun|];   -- nobody, anywhere, on purpose
```

**`restricted`** means the variable exists, it holds its value, and nothing is allowed
to touch it. The declaration compiles. Every use of it does not.

You can say it out loud, as above. It is also what you get by saying nothing:

```luarust
var.str ['name'] = [|Tankun|];   -- declared, and unusable
```

This is a joke, and it is also the default, so if you would rather hear about it where
you wrote it than where you used it, say so at the top of the file:

```luarust
defaults.no-visibility-stated.error;
```

and a declaration that states no visibility becomes an error on the spot.

## The project file

A Luarust source file is `.lr`. Settings for a whole project live beside them in a
`Luarust.toml`:

```toml
[defaults]
no-visibility-stated = "error"
overflow = "trap"

[build]
embed-source = false
```

Anything under `[defaults]` applies to every file in the project, so a preference you
hold everywhere is written once. A `defaults.` line at the top of a file still wins for
that file — whatever a file says about itself is the last word on it.

`[build]` is about what gets delivered rather than what gets accepted. `embed-source`
decides whether a compiled chunk carries the text it was built from. With it off, the
chunk carries the line table instead — four bytes per line — so a fault still reports
its exact line and column and simply cannot quote them:

```
file: src/main.lr, line: 3, column: 25 (src/main.lr:3:25)

this does not fit in `ui8`.

  (this was built without its source, so the line above cannot be shown.)
```

Luarust reads this file itself rather than through a TOML library, and understands the
part of it shown here. A mistake in it stops the build and is reported the same way a
mistake in a source file is, because it decides how every file in the project is built.

## What ships

Nothing goes onto the machine that runs a program unless that program uses it. There is
no garbage collector, and a program in integers and the hardware floats never allocates
at all: the only heap in a value is the one strings and the wide floats need.

That rule is why there are two binaries and not one.

| | stripped | what it is |
|---|---|---|
| `luarust` | 712 KB | the toolchain — lexes, parses, checks, compiles, runs, disassembles |
| `luarust-run` | 461 KB | a chunk, and nothing else |
| `luarust` with the JIT | 32 MB | the above plus LLVM |

`luarust-run` takes a `.lrc` and runs it. It cannot compile, because a chunk already is
compiled, and it has no lexer, parser, checker, program generator or JIT linked into it
at all — those are facts about writing Luarust, not about running it. LLVM is fifty times
the size of everything in this repository put together, which is the clearest possible
argument that a JIT is a development tool and never part of what you hand somebody.

## Types

```
b16  b32  b64  b128  b256     IEEE 754 binary floats
d32  d64  d128                IEEE 754 decimal floats
er                            exact rational, unbounded
i8   i16  i32  i64            signed integers
ui8  ui16 ui32 ui64           unsigned integers
bool                          true or false
str                           text
```

Luarust is a floating-point language that happens to contain integers, rather than the
other way round. They are there because counting and indexing want them, and like every
float here they state their width in their name — there is no bare `i` you have to look
up. Everything else numeric is a float, and `er` is an exact rational — a numerator over
a denominator, both unbounded, so it neither rounds nor overflows.

The binary formats are the real IEEE 754 ones rather than approximations of them:
`b16` is true half precision, and `b256` carries 237 bits of significand, which almost
nothing else on earth implements. The decimal formats are the ones where `0.1` is
exactly `0.1` and money keeps its cents.

Two of the eight float types — `b32` and `b64` — are formats the hardware knows. The
rest Luarust computes itself, to the rounding the standard requires.

### Overflow

A float never overflows into an error. IEEE 754 answers with ±infinity, which is a value
like any other, and Luarust hands it to you.

An integer wraps. Add one to the largest `ui8` and it is zero again. If you would rather
be told about it, say so:

```luarust
defaults.overflow.trap;
```

or once for the whole project:

```toml
[defaults]
overflow = "trap"
```

## Errors

An error apologises for the interruption, points at the code, names the rule that was
broken, and finishes on the fix — so the last thing left on screen is what to do next.

```
Hello, I think there may be thing(s) wrong with your code. I'm sorry, if I'm wrong.

file: /Users/ts/hello/src/main.lr, line: 4, column: 6 (src/main.lr:4:6)

`'total'` cannot be changed, because its declaration never said it could.

  2 | var.local.ui32 ['total'] = [|0|];
    |     ~~~~~ declared here, and `mut` is not in the chain
  4 | set ['total'] = [|55|];
    |      ^^^^^^^ changed here

Error code: E0104
Rule(s) broken: a variable changes only if its declaration says `mut`
Tip(s): `mut` goes between the visibility and the type.
Suggested fix(s): line 2 — `var.local.mut.ui32 ['total'] = [|0|];`

1 error.
```

The greeting is printed once however many errors follow it, and the count once at the end.

**Columns are counted the way a reader counts.** `🧑‍🧑‍🧒‍🧒` is one character, exactly as `c`
is, though it is seven Unicode scalars welded together with zero-width joiners and
twenty-five bytes on disk. The short `file:line:column` in brackets carries a **byte**
column instead, because its job is to be pasted into an editor or a `grep`, and that is
the number those understand.

Carets are laid out in **terminal cells**, which is a third measurement again — an emoji
draws two cells wide where a letter draws one. The number you read and the caret you see
are counted differently on purpose, and both have to be right.

Every error names a rule. The message says what went wrong here; the rule says what is
true everywhere.

## Using it

```bash
cargo build --release
```

```bash
./target/release/luarust run examples/counting.lr
```

| command | what it does |
| --- | --- |
| `luarust run <file.lr>` | compile to bytecode and run it |
| `luarust interp <file.lr>` | run it on the reference interpreter instead |
| `luarust verify <file.lr>` | run it both ways and report whether they agree |
| `luarust build <file.lr>` | compile it to a `.lrc` chunk and stop |
| `luarust dis <file.lr>` | show what the compiler decided |
| `luarust check <file.lr>` | check it and stop |
| `luarust fuzz [count]` | write programs and check the paths agree |
| `luarust jit <file.lr>` | compile it with LLVM, in memory, and run it |
| `luarust ir <file.lr>` | show the LLVM IR |
| `luarust-run <file.lrc>` | run a chunk, and nothing else |

The programs in [`examples/`](examples) all run, and are checked on every push —
including [`fizzbuzz.lr`](examples/fizzbuzz.lr), which is really a test of `if`.

`luarust-run` is a separate binary on purpose — see [What ships](#what-ships).

### Compile once, run anywhere

`luarust build` writes a **`.lrc` chunk**: the whole program, in one file, with no source
and no compiler needed to run it.

```bash
luarust build hello.lr      # hello.lrc — 514 bytes
luarust run hello.lrc
```

Everything in it is little-endian whatever machine wrote it, so a chunk built on one
architecture runs on another. **The source travels inside it** unless `[build]
embed-source` says otherwise, which costs a few kilobytes and buys the thing that would
otherwise be lost — a program that stops half way through can
still point at the line that did it, on a machine that has never seen the source:

```
file: /tmp/faulty.lr, line: 3, column: 21 (/tmp/faulty.lr:3:21)

this divides a whole number by zero.

  3 | set ['n'] = [math { 'n' div 0 }];
    |                     ^^^^^^^^^ while running this
```

That file had been deleted before the chunk was run.

**Nothing read from a chunk is trusted.** Every register, constant, text and jump target is
checked against what it indexes before the program starts, because a corrupt file has to
produce a complaint rather than a crash — and "run anywhere" means chunks arrive from
places nobody vouched for. There is a test that flips every bit of every byte of a chunk
and requires that each one either loads or explains itself.

### Three ways to run one

There are **three ways to run a program**, and that is deliberate. `jit` compiles to
machine code with LLVM, in memory; `run` compiles to bytecode; `interp` walks the checked
tree directly, doing no compilation at all. The tree-walker is slow and is staying: it is
the reference the other two answer to.

One implementation only ever agrees with itself. `luarust verify` runs a program two ways
and says whether they match, and `luarust fuzz` writes programs and does it in bulk — a
million of them at the last count, all compiling, all agreeing, and the fourteen thousand
that stopped part way stopping the same way both times.

The JIT takes every program, but it does not compile all of every program. Integers,
`b32` and `b64` become native instructions, because those are the cases where LLVM's
arithmetic and Luarust's own are both correctly rounded and so cannot differ. The rest —
`b16`, `b128`, `b256`, `bool`, `str`, and raising to a power — go back to `luarust-num` by
way of a call, and `b128`, `b256`, `bool` and `str` do not travel in registers at all: they
live in numbered cells on the Rust side and the machine code carries the number.

That is not a compromise. Their arithmetic was always going to be a call, because none of
those formats has hardware anywhere and their answers have to come from the same place the
other two execution paths get theirs. An answer the three paths might disagree about is
worth less than no answer.

The JIT needs LLVM 21 and is behind a feature, so everything else builds with no
dependencies at all:

```bash
cargo build --release -p luarust-cli --features jit
```

## How fast it is

The benchmark is a dependent chain — `sum = (sum + i) mod 1000000007`, a hundred million
times, in a signed 64-bit integer. Each value needs the one before it, so it cannot be
folded into a formula, vectorised, or run out of order. Everybody actually loops.

| | 100M | vs C |
| --- | --- | --- |
| C, clang -O2 | 376 ms | 1× |
| Rust, rustc -O | 392 ms | 1.04× |
| Java 17 | 413 ms | 1.10× |
| **Luarust**, LLVM JIT | **479 ms** | **1.27×** |
| Lua 5.4 | 751 ms | 2.0× |
| LuaJIT | 796 ms | 2.1× |
| Lust | 1,053 ms | 2.8× |
| Luarust, bytecode VM | 4,868 ms | 12.9× |
| Luarust, tree-walker | 11,865 ms | 31.6× |

One x86-64 machine, one job, best of three, every one of them printing 15000000.

Some of that 480 ms is LLVM compiling the program, which happens inside the measurement.
And some of the gap to C is a feature rather than a shortfall: a Luarust loop tests
whether the counter has *reached* its bound before stepping, rather than stepping and then
testing, which is what lets `['253', '255']` finish in a `ui8` instead of wrapping round
forever. That is one extra comparison per iteration, on purpose.

Before any of it means anything, each timing is checked for whether it still contains a
loop at all — a compiler that spots the sum of 1 to n and replaces the whole thing with a
formula reports a magnificent number for doing nothing. Ten times the work should take ten
times the time:

| | 10M | 100M | ratio |
| --- | --- | --- | --- |
| C, clang -O2 | 40 ms | 376 ms | 9.4× |
| Rust, rustc -O | 42 ms | 392 ms | 9.3× |
| Java 17 | 80 ms | 413 ms | 5.2× |
| Luarust JIT | 58 ms | 479 ms | 8.3× |
| Lua 5.4 | 78 ms | 751 ms | 9.6× |
| LuaJIT | 82 ms | 796 ms | 9.7× |
| Lust | 109 ms | 1,053 ms | 9.7× |
| Luarust VM | 494 ms | 4,868 ms | 9.9× |
| Luarust tree-walker | 1,205 ms | 11,865 ms | 9.8× |

Nobody's loop was deleted. The three that come in under ten are the three that pay a fixed
cost before they start: the JVM starting, and LLVM compiling.

Everything reads its `N` from the command line so it cannot be folded away, except Luarust,
which has no argv yet and has it written into the source. The scaling table is what would
catch that if it ever began to matter.

The `benchmark` workflow in this repository runs both tables. The numbers move by a third
between runs because the machine underneath is shared, which is why every row is always
measured in the same job as every other.

# Luarust
Project under development, stable use not recommended, stability is **not a guarantee**

**Luarust** is a **Lua ripoff** focused on performance, device support, explicit syntax, and very helpful error messages (unlike fucking `C`, joking 😂).

Luarust's method is focused on compile once, run anywhere (Like Java's), and boosting its performance via JIT compilation, with **LLVM** doing the code generation.

So, to put it simply. Luarust is a computer language that has **very helpful error messages**, and **could run basically anywhere**, **without sacrificing much performance**.

## Declaring things

A declaration says the scope and the type up front, then takes a list of names and a
list of values. Statements end with a semicolon.

```luarust
var.local.str ['name'] = ['Tankun'];
```

Names live in quotes, so a name can be anything you can type — spaces, punctuation,
emoji, whatever you actually wanted to call it:

```luarust
var.local.str ['a friendly greeting'] = ['hello'];
var.local.b16 ['❔']                  = ['1000'];
```

Values live in quotes too, and the type is what decides how one reads. The same four
characters are a number under `b16` and text under `str`:

```luarust
var.local.b16 ['x'] = ['1000'];    -- the number 1000
var.local.str ['y'] = ['1000'];    -- the text "1000"
```

### Several at once

The brackets hold lists, so one declaration can make several variables that share a
scope and a type:

```luarust
var.local.str ['name', 'name 2', 'name 3'] = ['Tankun', 'Ada', 'Jensen Huang'];
```

Whatever they have in common hoists onto `var`, and whatever they don't goes inline.
Same scope, different types:

```luarust
var.local [str 'a', b16 'b'] = ['hello', '1000'];
```

Nothing in common at all:

```luarust
var [local.str 'name', local.b16 'x', local.str '❔'] = ['Tankun', '1000', 'idk'];
```

The two lists have to be the same length. Three names and two values is an error, and
it names the one that went without — Luarust will not invent a value you did not
write.

## Changing things

`var` makes a variable. `set` changes one that already exists:

```luarust
var.local.mut.b16 ['x'] = ['1000'];
set ['x'] = [math { 'x' + 1 }];
```

A variable cannot be changed unless it said it could. `mut` goes in the chain, and
without it that second line is an error — one that names the word you left out rather
than the line you wrote it on.

`set` carries no visibility and no type, because the variable already has both. It takes
the same lists as everything else:

```luarust
var.local.mut.b16 ['a', 'b'] = ['1', '2'];
set ['a', 'b'] = ['10', '20'];
```

`mut` hoists like the rest of the chain, so names that share it say it once:

```luarust
var.local.mut [b16 'a', b64 'b'] = ['1', '2'];
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
var.local.b16 ['x'] = ['1000'];
print["x is equal to " 'x' \n];
```

Double quotes are text. **Single quotes are a name** — `'x'` is the variable, not the
letter x. That is how a value gets read back out of one.

`\n` is written as a bare token outside the quotes, and nothing is inserted on your
behalf: no separator between items, and no line ending unless you write one.

A number prints as the value that is actually stored, not as the text that was written
to make it. `b16` carries eleven bits of significand, and most decimals are not in it:

```luarust
var.local.b16 ['a'] = ['0.1'];
print['a' \n];                 -- 0.0999755859375
```

That *is* `b16 '0.1'` — the nearest `b16` to a tenth, exactly. Luarust will not print a
tidier number than the one it is holding.

## Arithmetic

Arithmetic happens inside `math { }` and nowhere else. A math block stands where a
value stands:

```luarust
var.local.b16 ['x', 'y'] = ['3', '4'];
var.local.b16 ['z']      = [math { 'x' + 'y' }];
print['z' \n];                                     -- 7
```

Single quotes mean here what they mean in a print list: `'x'` is the variable. Numbers,
though, are written bare:

```luarust
var.local.b16 ['z'] = [math { 'x' + 1 }];
```

Quotes exist so that a type annotation can decide what a literal means — `'1000'` is a
number under `b16` and text under `str`. Inside a math block there is nothing left to
decide, so the quotes come off. A bare number takes its type from its surroundings, and
`'1'` in a math block would be a variable named `1`.

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

`%` is **not** remainder. Mathematics has never used it for one — it is percent, written
after a number, so `15%` is fifteen hundredths:

```luarust
var.local.d64 ['price'] = ['19.99'];
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

Words work as operators here because a name is always quoted. `'x'` is a variable and a
bare `x` cannot be one, so there is nothing for `math { 'a' x 'b' }` to be confused with.

That is five kinds of bracket, and the reason it works is that no two of them ever
mean the same thing:

| | |
| --- | --- |
| `[ ]` | a list — of names, of values, or of things to print |
| `{ }` | a block — the word in front of it says which kind |
| `( )` | grouping, inside a math block |
| `' '` | a name — or a literal, where a value is expected and no math block is open |
| `" "` | text |

You never have to work out which sense a bracket is being used in. It only has one.

## Loops

A loop is written the way a declaration is, because it does the same thing a declaration
does: a chain saying how long the counter lives, what kind of loop it is, and what type
the counter has — then a name, then the values that set it going.

```luarust
loop.temp.range.ui8 ['i'] = ['1', '5'] {
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
loop.temp.range.ui8 ['i'] = ['1', '5'] { … }    -- 'i' is gone at the brace
loop.perm.range.ui8 ['i'] = ['1', '5'] { … }    -- 'i' is still there afterwards
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
var.local.mut.ui32 ['total'] = ['0'];

loop.temp.range.ui32 ['i'] = ['1', '10'] {
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
var.local.mut.ui64 ['sum'] = ['0'];
var.local.b64 ['start']    = [time.now];

loop.temp.range.ui64 ['i'] = ['1', '100000000'] {
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
var.local.str      ['name'] = ['Tankun'];   -- the block it is written in, and no further
var.global.str     ['name'] = ['Tankun'];   -- the whole program
var.public.str     ['name'] = ['Tankun'];   -- and exported, so importers see it too
var.restricted.str ['name'] = ['Tankun'];   -- nobody, anywhere, on purpose
```

**`restricted`** means the variable exists, it holds its value, and nothing is allowed
to touch it. The declaration compiles. Every use of it does not.

You can say it out loud, as above. It is also what you get by saying nothing:

```luarust
var.str ['name'] = ['Tankun'];   -- declared, and unusable
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
```

Anything under `[defaults]` applies to every file in the project, so a preference you
hold everywhere is written once. A `defaults.` line at the top of a file still wins for
that file — whatever a file says about itself is the last word on it.

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

  2 | var.local.ui32 ['total'] = ['0'];
    |     ~~~~~ declared here, and `mut` is not in the chain
  4 | set ['total'] = ['55'];
    |      ^^^^^^^ changed here

Error code: E0104
Rule(s) broken: a variable changes only if its declaration says `mut`
Tip(s): `mut` goes between the visibility and the type.
Suggested fix(s): line 2 — `var.local.mut.ui32 ['total'] = ['0'];`

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
| `luarust dis <file.lr>` | show what the compiler decided |
| `luarust check <file.lr>` | check it and stop |
| `luarust fuzz [count]` | write programs and check the paths agree |
| `luarust jit <file.lr>` | compile it with LLVM, in memory, and run it |
| `luarust ir <file.lr>` | show the LLVM IR |

The programs in [`examples/`](examples) all run, and are checked on every push.

There are **three ways to run a program**, and that is deliberate. `jit` compiles to
machine code with LLVM, in memory; `run` compiles to bytecode; `interp` walks the checked
tree directly, doing no compilation at all. The tree-walker is slow and is staying: it is
the reference the other two answer to.

One implementation only ever agrees with itself. `luarust verify` runs a program two ways
and says whether they match, and `luarust fuzz` writes programs and does it in bulk — a
million of them at the last count, all compiling, all agreeing, and the fourteen thousand
that stopped part way stopping the same way both times.

The JIT **declines** more programs than it takes, on purpose. Integers and `b32`/`b64`
become native instructions, because those are the cases where LLVM's arithmetic and
Luarust's own are both correctly rounded and so cannot differ. `b16`, `b128`, `b256`,
powers, `bool` and `str` are handed back, and the bytecode VM runs them instead. An answer
the three paths might disagree about is worth less than no answer.

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
| C, clang -O2 | 377 ms | 1× |
| Rust, rustc -O | 392 ms | 1.04× |
| Java 17 | 419 ms | 1.11× |
| **Luarust**, LLVM JIT | **480 ms** | **1.27×** |
| Lua 5.4 | 753 ms | 2.0× |
| LuaJIT | 797 ms | 2.1× |
| Lust | 1,054 ms | 2.8× |
| Luarust, bytecode VM | 7,785 ms | 20.6× |
| Luarust, tree-walker | 13,437 ms | 35.6× |

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
| C, clang -O2 | 40 ms | 377 ms | 9.4× |
| Rust, rustc -O | 42 ms | 392 ms | 9.3× |
| Java 17 | 80 ms | 419 ms | 5.2× |
| Luarust JIT | 60 ms | 480 ms | 8.0× |
| Lua 5.4 | 79 ms | 753 ms | 9.5× |
| LuaJIT | 83 ms | 797 ms | 9.6× |
| Lust | 110 ms | 1,054 ms | 9.6× |
| Luarust VM | 786 ms | 7,785 ms | 9.9× |
| Luarust tree-walker | 1,352 ms | 13,437 ms | 9.9× |

Nobody's loop was deleted. The three that come in under ten are the three that pay a fixed
cost before they start: the JVM starting, and LLVM compiling.

Everything reads its `N` from the command line so it cannot be folded away, except Luarust,
which has no argv yet and has it written into the source. The scaling table is what would
catch that if it ever began to matter.

The `benchmark` workflow in this repository runs both tables. The numbers move by a third
between runs because the machine underneath is shared, which is why every row is always
measured in the same job as every other.

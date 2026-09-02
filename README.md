# Luarust
Project under development, stable use not recommended, stability is **not a guarantee**

**Luarust** is a **Lua ripoff** focused on performance, device support, explicit syntax, and very helpful error messages (unlike fucking `C`, joking 😂).

Luarust's method is compile once to a `.lrc` that runs anywhere (Like Java's), and the project picks how it runs there: the **bytecode VM**, or the **whole-chunk JIT**, or the **hot JIT** that starts interpreting and compiles a loop once it proves itself. **Native** output trades the anywhere for a binary that needs nothing on the machine it lands on. **LLVM** does the code generation. The final product only contains what is needed (unlike fucking `Go`, joking, again 😂)

Putting it simply: **Luarust** is/will be a language that **could be compiled in many methods**, so you could **choose what is best for you**.

### Why pick Luarust, and why not
| Why pick Luarust | Why not |
| --- | --- |
| Compile once, run basically anywhere | One of the slowest languages |
| High-level | No direct electricity manipulation |
| Final product only has the things that are needed (once again, unlike fucking `Go` 🤣) | It doesn't give you everything (e.g. time manipulation) |
| Very helpful error messages (hahaha, guess what? Unlike fucking `C` 🤣) | Error messages may be too long |
| | Stability is not guaranteed |

### Why pick Luarust, and why not (no jokes)

| Why pick it | Why not |
| --- | --- |
| An error names the rule that was broken and the fix, points at the line, and apologises for the interruption | That is a lot of polish on a language with no standard library, no `sqrt`, and no way to read a file or take an argument |
| Nothing is implicit — no conversion, no truthiness, no coercion. Two types meet only where something said they should | Saying so is wordy. `var.local.mut.ui32 ['total'] = [\|0\|];` is a lot of characters for a counter |
| Real IEEE 754, correctly rounded, in five binary formats and three decimal ones — including `b256` and `d128`, which almost nothing else on earth implements — and `er`, which never rounds at all. A float prints as the value it holds, exactly, so `b64 \|0.1\|` shows you it is not one tenth | Nothing is built on top of them. The tower is wide and the library on it is empty. And a `b256` prints more digits than the literal parser can read back |
| Arrays are stored as arrays: packed by element width, so ten million `ui8`s take ten million bytes, and compiled code reads one with a load rather than a call | An array holds scalars, so there are no arrays of arrays and nothing in the language can contain itself |
| Compile once, run anywhere is literal: one `.lrc` file, little-endian everywhere, and a **461 KB** runtime to run it on | Building the JIT needs LLVM 21 exactly — 20 works but is unsupported, because a code generator CI never runs cannot be promised to agree with the other two paths |
| Three implementations — a tree-walker, a bytecode VM, and an LLVM JIT — that must agree bit for bit on 200,000 generated programs before anything ships | Three implementations is also three places for a bug to hide. The fuzzer found one where `0` and `-0` shared a constant slot, which had been there for as long as the pool had |
| Only what a program uses gets delivered, and `[gc] mode` is off until asked for — a program that makes no arrays carries no collector at all | There is not much to leave out yet, so this is a promise about the future as much as a fact about now |
| It is small enough to read. Thirteen crates, about 20,500 lines, and the whole thing fits in a head | It is one person's hobby project at version 0.0.0, and stability is **not a guarantee** |

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

Written values wear bars instead — or backticks, whichever your keyboard likes — so a
quoted thing is a name wherever you meet one and never has to be read as a value
depending on where it sits. The type is what decides how
a written value reads — the same four characters are a number under `b16` and text under
`str`:

```luarust
var.local.b16 ['x'] = [|1000|];    # the number 1000
var.local.str ['y'] = [|1000|];    # the text "1000"
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
handback 'i' as 'total';                     # the same thing
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
print['a' \n];                 # 0.0999755859375
```

That *is* `b16 '0.1'` — the nearest `b16` to a tenth, exactly. Luarust will not print a
tidier number than the one it is holding.

## Arithmetic

Arithmetic happens inside `math { }` and nowhere else. A math block stands where a
value stands:

```luarust
var.local.b16 ['x', 'y'] = [|3|, |4|];
var.local.b16 ['z']      = [math { 'x' + 'y' }];
print['z' \n];                                     # 7
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
var.local.b16 ['w'] = [math { ('x' + 'y') * 'x' }];   # 21
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
math { -7 mod 3 }     # 2,  not -1
math { 7 mod -3 }     # -2, not 1
```

So `'i' mod 3` cycles `0 1 2` however `'i'` is signed, which is what anyone counting
actually wanted.

`div` rounds to match it, because a quotient and a remainder describe one division:
`(a div b) x b + (a mod b)` is `a`, whichever way it rounds. The three ways that hold
that together are all here, and the project picks one:

| `[defaults] division` | `-7 div 3` | `-7 mod 3` | `7 div -3` | `7 mod -3` | the remainder |
|---|---|---|---|---|---|
| `"floored"`, the default | `-3` | `2` | `-3` | `-2` | follows the divisor |
| `"truncated"` | `-2` | `-1` | `-2` | `1` | follows the dividend, as in C |
| `"euclidean"` | `-3` | `2` | `-2` | `1` | is never negative |

Unsigned types cannot tell them apart, and only whole numbers round at all: `div` on a
float, a decimal or an `er` is exact division, so there the setting decides `mod` alone.

### Comparing

Six of them, and they answer `bool`:

```luarust
var.local.i32 ['a', 'b'] = [|3|, |5|];

var.local.bool ['less']    = [math { 'a' <   'b' }];    # true
var.local.bool ['more']    = [math { 'a' >   'b' }];    # false
var.local.bool ['at most'] = [math { 'a' </= 'a' }];    # true
var.local.bool ['same']    = [math { 'a' =   'a' }];    # true
var.local.bool ['differs'] = [math { 'a' !=  'b' }];    # true
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
| `\| \|` or `` ` ` `` | a written value — always |
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

## Arrays

```luarust
var.local.mut.array.5.ui32 ['xs'] = [[|10|, |20|, |30|, |40|, |50|]];

print['xs'[|1|] "   " count['xs'] \n];    # 10   5
set ['xs'[|3|]] = [|99|];

var.local.array.2x3.ui8 ['m'] = [[
    |1|, |2|, |3|,
    |4|, |5|, |6|
]];
print['m'[|2|, |3|] \n];                   # 6
```

`array.ui32` grows; `array.5.ui32` is five for ever; `array.2x3.ui8` is two rows of
three. A shaped one is written flat, row by row, because the type already said the shape
and saying it twice would only be a chance to disagree.

**Counted from one.** The first element is `1` and `0` is no element at all. That is not a
preference — it falls out of two decisions already made. The counting loop is inclusive
and its counter is usually unsigned, so this walks an array exactly:

```luarust
loop.temp.range.ui32 ['i'] = [|1|, count['xs']] { … }
```

Counting from nought would need `[|0|, count - 1|]`, and on an empty array `count - 1`
wraps round to eighteen quintillion.

A **quoted** name before a bracket is an index; a **bare** word before one is a call. That
distinction was already in the language, so `'xs'[|1|]` and `double[|5|]` cannot be
confused. `count[…]` answers in whatever type is expecting it, and `filled[…]` makes one —
a fixed array is told only what to fill with, since it already knows how many.

### They are stored as arrays

A thousand `ui8`s take a thousand bytes. The elements are packed by width — one byte for
`ui8` and `bool`, two for `ui16`, four for `ui32`, eight for `ui64` — rather than being a
run of values, each sixteen bytes and each carrying a type the array already stated.

| a thousand of | as values | packed |
| --- | --- | --- |
| `ui8` | 16,000 bytes | 1,000 |
| `ui32` | 16,000 | 4,000 |
| `b64` | 16,000 | 8,000 |

The memory is the smaller half of it. The real reason is that packed elements make
indexing **arithmetic**: element `n` is at `base + n × width`, which the JIT emits as a
load. Out of a run of tagged values it could emit nothing at all — reading one would mean
calling back into Rust to open a box and ask what was in it.

A value holds a **handle** into the heap, which fits in the space a number uses. So an
array costs nothing to have around: no new kind of value, and none of the drop-checking
that a reference-counted one would put on every assignment in the language.

### Collecting them

A function takes one and answers with one, written the way any other type is:

```luarust
fn.local.array.ui32 ['doubled'] [array.3.ui32 'xs'] {
    var.local.mut.array.ui32 ['out'] = [filled[|3|, |0|]];
    loop.temp.range.ui32 ['i'] = [|1|, |3|] {
        set ['out'['i']] = [math { 'xs'['i'] + 'xs'['i'] }];
    }
    return 'out';
}
print["doubled " doubled[[|1|, |2|, |3|]] \n];   # doubled [2, 4, 6]
```

Nothing is copied. A handle is what travels, so passing a hundred thousand elements costs
what passing a number costs.

Nothing frees an array on its own, so a program that makes one every time round a loop
grows for as long as it runs. The collector is what stops that, and it is **off unless
asked for**:

```toml
[gc]
mode = "silent"

[run]
mode = "hot"
```

`"off"` never collects, `"silent"` collects when a megabyte has been handed out since the
last time, and `"aggressive"` does it every four kilobytes. Two hundred thousand arrays of
two hundred `ui64`, made and forgotten one at a time:

| `[gc] mode` | peak memory |
| --- | --- |
| `"off"` | 359.7 MB |
| `"silent"` | 3.3 MB |
| `"aggressive"` | 2.0 MB |

A program that makes no arrays pays nothing for a collector either way, because what it
costs to ask is a load and a compare on the line that makes an array, and that line never
runs.

It is **mark and sweep**, and nothing more elaborate is needed. An array's elements are
scalars, so no value in this language can contain itself and there are no cycles to chase;
what no root can reach is garbage, and a reference count would have found exactly the same
set. A swept slot keeps its index, because a handle *is* an index, and gives up its
elements, because that is where the memory was.

**Compiled code collects too**, and not by the usual route. LLVM's `gc.statepoint`
machinery tracks *pointers* and relocates them from a stack map; a Luarust handle is an
*index*, so there is nothing for a stack map to relocate and none of that applies.

What compiled code does instead is keep a copy. Register `n` already has cell `n` for
values machine code cannot hold, and a handle rides in a register perfectly well — so it
is written to its cell as well, and the frames become the root set. That is one store per
handle written, and handles are written when an array is made or moved, never in the loop
that reads one.

| a loop making 200,000 arrays | `"off"` | `"silent"` |
| --- | --- | --- |
| VM | 359.7 MB | 3.3 MB |
| JIT | 385.3 MB | 29.0 MB |

The JIT's floor is higher because the process is carrying LLVM.

It is **conservative**: a cell whose register has since been reused for something else
keeps its old array alive. That holds on to something dead and never frees something
live, which is the right way round for that trade to fall.

**The tree-walker does not collect**, and cannot as it stands. Its intermediate values
live in Rust locals up the recursion — the vector being filled for an array that is
half-built — where nothing can enumerate them. It is the oracle rather than something
anybody ships, so it keeps every array it makes and that costs nothing that matters.

None of that can make the paths disagree, because **collecting changes no output**. So
`luarust fuzz` runs with the VM sweeping every four kilobytes and the other two never
sweeping at all: if a collector ever freed something a program could still reach, it
would show up there as a disagreement rather than as a wrong answer months later.

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

### Looping until something changes

A counting loop needs both its bounds before it starts. When you do not know how many
times, say what has to keep being true instead:

```luarust
loop.while [math { 'delta' > b64 |0.000001| }] {
    …
}
```

The condition is asked again before every pass. There is no `temp`/`perm` in the chain,
because there is no counter to say it about — until you ask for one, and then there is:

```luarust
loop.temp.while.ui32 ['pass'] [|true|] {
    print["pass " 'pass' \n];
    break when reached |3|;
}
```

`pass` is 1 the first time round, counted at the start of each pass — so afterwards it
holds however many ran, not one more than that. A loop whose condition was false from the
start leaves it at nothing. It is a real variable like any other: print it, do arithmetic
with it, keep it after the loop with `perm`.

**`break` leaves the innermost loop**, in a counting loop as much as a conditional one:

```luarust
loop.temp.range.ui32 ['i'] = [|1|, count['xs']] {
    if [math { 'xs'['i'] > ui32 |100| }] {
        set ['found'] = ['i'];
        break;
    }
}
```

`break when reached |7|;` is the same thing with the `if` folded in — it compares against
the counter of the loop it is in, so it is an error in a `while` loop that never asked for
one. The message says exactly that rather than guessing.

### How long the counter lives

`temp` and `perm` say whether the counter outlives the loop, and one of them has to be
said:

```luarust
loop.temp.range.ui8 ['i'] = [|1|, |5|] { … }    # 'i' is gone at the brace
loop.perm.range.ui8 ['i'] = [|1|, |5|] { … }    # 'i' is still there afterwards
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
var.local.str      ['name'] = [|Tankun|];   # the block it is written in, and no further
var.global.str     ['name'] = [|Tankun|];   # the whole program
var.public.str     ['name'] = [|Tankun|];   # and exported, so importers see it too
var.restricted.str ['name'] = [|Tankun|];   # nobody, anywhere, on purpose
```

**`restricted`** means the variable exists, it holds its value, and nothing is allowed
to touch it. The declaration compiles. Every use of it does not.

You can say it out loud, as above. It is also what you get by saying nothing:

```luarust
var.str ['name'] = [|Tankun|];   # declared, and unusable
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
float-printing = "exact"
division = "floored"

[build]
embed-source = false
decimal-encoding = "dpd"
target-cpu = "portable"

[gc]
mode = "silent"
```

Anything under `[defaults]` applies to every file in the project, so a preference you
hold everywhere is written once. A `defaults.` line at the top of a file still wins for
that file — whatever a file says about itself is the last word on it.

Comments are `#`, as in any TOML file, at the start of a line or after a value.
`luarust check Luarust.toml` reads it as a project file and says what is wrong with it.

`luarust native file.lr --for x86_64-unknown-linux-gnu` builds the program for a machine
that is not this one. LLVM writes the object for whichever target it was built with, so
that half costs nothing; the rest of it needs two things the language cannot supply for
you — the runtime archive built for that target, and a linker that can finish the job:

```bash
cargo build --release -p luarust-native --target x86_64-unknown-linux-gnu
luarust native hello.lr --for x86_64-unknown-linux-gnu
```

`zig cc` is looked for first because it carries a libc for every target it knows, and a
`<triple>-gcc` after it. When neither is there, or the runtime for that target has never
been built, it says which of the two is missing and what to run — rather than writing an
object and leaving you with it.

`target-cpu` decides which machine `luarust native` is building a program *for*. It is
`"portable"` by default — everything the architecture guarantees, and nothing this
particular processor happens to add — because native output is for the machine that will
run it, and that machine is not this one unless somebody says so. `"this-machine"` uses
everything the builder has, which is faster and runs only on a processor at least as
capable — and is ignored when `--for` names somewhere else, since naming this machine's
processor is only truthful when this machine is the one that will run it. Getting this wrong is not a slow program: it is an illegal instruction on the
first one the target does not implement.

`[build]` is about what gets delivered rather than what gets accepted. `embed-source`
decides whether a compiled chunk carries the text it was built from. With it off, the
chunk carries the line table instead — four bytes per line — so a fault still reports
its exact line and column and simply cannot quote them:

```
file: src/main.lr, line: 3, column: 25 (src/main.lr:3:25)

this does not fit in `ui8`.

  (this was built without its source, so the line above cannot be shown.)
```

A chunk carries a sum of its own bytes, checked before a single field of it is believed.
That is for accidents — a bad copy, a transfer that stopped early, a disk going wrong —
and it says so rather than range-checking a damaged file field by field into some plausible
program nobody wrote. It is **not** a defence: anyone editing a chunk on purpose
recomputes it in a line. What a chunk claims about *itself* is a different question.

Luarust reads this file itself rather than through a TOML library, and understands the
part of it shown here. A mistake in it stops the build and is reported the same way a
mistake in a source file is, because it decides how every file in the project is built.

## What ships

Nothing goes onto the machine that runs a program unless that program uses it. There is
no garbage collector unless `[gc] mode` asked for one, and a program in integers and the
hardware floats never allocates at all: the only heap in a value is the one strings and
the wide floats need.

That rule is why there are two binaries and not one.

| | stripped | what it is |
|---|---|---|
| `luarust` | 712 KB | the toolchain — lexes, parses, checks, compiles, runs, disassembles |
| `luarust-run` | 461 KB | a chunk, and nothing else |
| `luarust` with the JIT | 32 MB | the above plus LLVM |

`luarust-run` takes a `.lrc` and runs it. It cannot compile, because a chunk already is
compiled, and it has no lexer, parser, checker or program generator linked into it at all
— those are facts about writing Luarust, not about running it.

A JIT it will carry, if asked: `cargo build --release -p luarust-run --features jit`. That
is not the same as carrying one on the chance, since a build that does not ask still has no
compiler in it. What it buys is that shipping a program which wants `"hot"` means shipping
a *runtime*, rather than the toolchain, which would put a lexer, a parser, a checker, a
disassembler and a program generator on a machine that only ever had to run one program.
It is also much smaller than the toolchain for a reason that is not the front end:
`luarust native` calls `Target::initialize_all` so it can build for a machine that is not
this one, so the toolchain links a code generator for **every architecture LLVM has**. A
runtime compiles for the machine it is standing on and wants exactly one.

It has no project file either, and never looks for one. A chunk carries what its project
decided — `overflow`, `[gc] mode`, `float-printing`, `division` — so a program keeps its
own answers wherever it is run, on a machine that has never seen the `Luarust.toml` it
was built under.

The JIT is a different case, and not a fourth thing left out of the runtime because it is
only for development. **How a shipped program runs is the project's choice**, written in
its project file and carried in the chunk:

```toml
[run]
mode = "vm"      # the bytecode, interpreted; nothing is compiled
mode = "whole"   # all of it through LLVM before it starts
mode = "hot"     # interpreted until a loop proves itself, compiled from there
```

And how much the project means it:

```toml
[run]
mode = "hot"
engine = "optional"   # no JIT on the machine? run on the VM, and say so — the default
engine = "required"   # no JIT on the machine? refuse to run
engine = "bundled"    # `luarust build` puts a runtime that has it beside the chunk
```

`"optional"` is what a chunk written before this setting existed means, and what one that
does not mention it means. It is the right default and it was for a while the only
behaviour, which is a different thing: a program that is unusable interpreted had no way to
say so and found out from its users.

`"bundled"` cannot conjure a runtime, any more than `luarust native` can conjure a target's
libc — something has to have built one. `luarust build` looks for a `luarust-run` beside
the toolchain, asks it what engines it has, and copies it only if the answer covers what
the chunk wants:

```bash
$ luarust build add.lr
add.lrc — 561 bytes
luarust-run — 55559552 bytes, and it can do `hot`
```

Asking is the only way to know. The two runtimes differ by a cargo feature and by nothing
visible in the file, which is what `luarust-run --engines` is for.

A program that starts, does a little and exits wants `"vm"`, where nothing is spent
compiling. A program that runs for hours wants `"whole"`, where a few milliseconds of LLVM
buys several times the speed for the rest of the day. Neither is the development answer
and the other the real one.

`"hot"` is the one that does not need to be told which it is. It interprets, counts how
often each loop goes round, and when one passes ten thousand it compiles **what that loop
can reach** and jumps into the middle of it with the registers the VM was holding.

It costs what the VM costs when nothing is worth compiling, because it never asks LLVM
for anything until a loop has already gone round ten thousand times. And it beats
compiling everything when only part of a program is worth compiling: `"whole"` compiles
every routine because the program might call any of them, while `"hot"` is asked from
inside one particular loop, and every call names its target, so it compiles the routines
that loop can reach and leaves the rest alone.

It is a **preference, not an instruction**. `luarust-run` has no JIT linked into it and is
not meant to, so a chunk asking for `"whole"` or `"hot"` runs on the VM there and says
nothing about it — refusing to run a program because the fastest way of running it is
unavailable would help nobody. The same happens when the JIT declines a program.

What the sizes above argue is narrower, and it is the same rule as everywhere else: a
program that will not use the JIT should not be carrying it. LLVM is fifty times the size
of everything in this repository put together, so *bundling it by default* would be the
whole of pay-for-what-you-use thrown away for the benefit of the programs that happen to
want it. Choosing it should cost 32 MB. Not choosing it should cost nothing.

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

### What a float prints

Exactly what it holds, and not the text that was typed to make it:

```luarust
var.local.b64 ['a'] = [|0.1|];
print['a' \n];        # 0.1000000000000000055511151231257827021181583404541015625
```

That is not a trick and nothing was rounded to produce it. `0.1` is not representable in
binary, so a `b64` holds the nearest value it has, and that value has the expansion above.
Every binary float has a finite one: it is `sig × 2^exp`, and a negative exponent is
`sig × 5^k / 10^k`, so the point simply goes `k` places along an integer.

Most languages print the shortest digits that would read back as the same number, which
shows `0.1` and lets you believe it. A language that would rather not guess should not —
so that is the default, and the other way is a setting, because both are true and they
only disagree about how much to say:

```toml
[defaults]
float-printing = "shortest"
```

| | `"exact"` | `"shortest"` |
| --- | --- | --- |
| `b64 \|0.1\|` | `0.1000000000000000055511151231257827021181583404541015625` | `0.1` |
| `b64 \|0.1\| + b64 \|0.2\|` | `0.3000000000000000444089209850062616169452667236328125` | `0.30000000000000004` |
| `b64 \|0.25\|` | `0.25` | `0.25` |
| `b128 \|1\| div b128 \|3\|` | 113 digits | 34 |

Neither ever shows a `b128` at a `b64`'s width, which was the bug both of them fixed.

| | prints |
| --- | --- |
| `b64 \|0.25\|` | `0.25` — exact in binary, so it stays short |
| `b32 \|0.1\|` | `0.100000001490116119384765625` |
| `b128 \|1\| div b128 \|3\|` | thirty-four significant digits, not a `b64`'s sixteen |
| `b256 \|1\| div b256 \|3\|` | seventy-two |
| `d64 \|0.1\|` | `0.1` — a decimal format holds a tenth exactly |
| `er \|1\| div er \|3\|` | `1/3` |

The one thing this outruns is reading. A `b256`'s expansion is around two hundred and
forty digits, and the literal parser accumulates digits into a fixed-width integer that
stops near a hundred and fifty — so what a `b256` prints is true and cannot be pasted back
into a program. The other four formats read back exactly.

### The one that never rounds

`er` is a numerator over a denominator, both unbounded, always in lowest terms. Nothing
in it rounds and nothing in it overflows:

```luarust
var.local.er ['a'] = [|0.1|];
var.local.er ['b'] = [|0.2|];
print[math { 'a' + 'b' } \n];       # 3/10, not 0.30000000000000004

var.local.er ['third'] = [|1/3|];
print[math { ('third' + 'third') + 'third' } \n];   # exactly 1
```

**A written `er` may be a fraction** — `|1/3|` — which no decimal could have said. A type
whose whole purpose is exactness should not make you approximate the first interesting
number. And it prints as a fraction for the same reason: a third has no finite decimal,
and writing `0.333…` is the one thing this type exists not to do.

Two things it will not do, and says so rather than guessing:

| | |
| --- | --- |
| `er \|2\| ** er \|1/2\|` | the square root of two is not a ratio |
| `er \|1\| div er \|0\|` | there is no infinity here to answer with |

What it costs: arithmetic allocates, and denominators grow when you add fractions with
nothing in common. Exactness is not free — it is just honest about what you are paying
for.

The binary formats are the real IEEE 754 ones rather than approximations of them:
`b16` is true half precision, and `b256` carries 237 bits of significand, which almost
nothing else on earth implements.

### The ones where money keeps its cents

`d32`, `d64` and `d128` are the IEEE 754 decimal formats. Their significands are decimal
digits — seven, sixteen and thirty-four of them — so a tenth is a tenth:

```luarust
var.local.d64 ['a'] = [|0.1|];
var.local.b64 ['x'] = [|0.1|];
print[math { 'a' + d64 |0.2| } \n];    # 0.3
print[math { 'x' + b64 |0.2| } \n];    # 0.30000000000000004

var.local.d64 ['price'] = [|19.99|];
print[math { 'price' x d64 |3| } \n];         # 59.97
print[math { d64 |20.00| - 'price' } \n];     # 0.01
```

They are **floats**, not exact rationals, and that is the difference between reaching for
`d64` and reaching for `er`. A decimal has a fixed number of digits, so a third rounds;
it has infinities, so dividing by zero answers `inf`; and it has NaNs. `er` has none of
those and no width either — it says no instead. Both are exact where the other is not:

| | `d64` | `er` |
| --- | --- | --- |
| `0.1 + 0.2` | `0.3` | `3/10` |
| `1 div 3` | `0.3333333333333333` | `1/3` |
| `1 div 0` | `inf` | an error |
| how wide | sixteen digits | as wide as it takes |

**`1.0` and `1.00` are different encodings of the same number** — a decimal carries its
exponent, so it remembers how it was written. They compare equal, because `=` asks what a
value is worth and not how it was spelled.

The standard gives two ways of writing the significand down: **BID** keeps it as a binary
integer, **DPD** packs three digits into every ten bits. They hold the same numbers, so
nothing about arithmetic depends on the choice — which is why it is a setting rather than
a decision:

```toml
[build]
decimal-encoding = "dpd"
```

Everything computes in BID, because that is what arithmetic wants. DPD is a repacking at
the edge, so choosing it costs nothing while a program runs and changes only the bytes in
the chunk.

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

## Comments

A comment begins with `#` and runs to the end of the line. A number straight after it says
how many lines, counting the one it is written on, and a `d` counts *down* from it instead:

```luarust
# this line
#3        this line and the two after it
#3d       this line and the three after it
```

So `#3` and `#2d` cover the same three lines, and a comment that reaches past the end of
the file simply ends there. `#0` is refused — nought lines is not a number of lines.

The number has to be written straight after the `#`, with nothing between, which is what
separates a count from a remark that begins with a number:

```luarust
# 3 things to fix here      this line
#3 things to fix here       this line and the two after it
```

Code before a `#` on the same line still runs, so a count of three from halfway down a
line means the rest of that line and two whole lines after it.

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
and no compiler needed to run it — and, since the JIT reads bytecode, no source needed to
compile it to machine code either.

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

There are **three ways to run a program**, and that is deliberate:

```
source ──lex, parse, check──▶ checked tree ──┬──▶ interp, walking it
                                             └──▶ bytecode ──┬──▶ VM, interpreting it
                                                             └──▶ LLVM ──▶ native
```

`interp` walks the checked tree directly, doing no compilation at all. It is slow and it
is staying: it is the reference the other two answer to. `run` compiles to bytecode and
interprets that. `jit` compiles the **same bytecode** to machine code with LLVM.

That last point is the one that matters. The JIT reads a chunk, so `.lrc` is not a file
that only the VM can run — it is the one artefact, and how fast it goes is a choice made
where it runs rather than where it was built:

```bash
luarust build hello.lr     # hello.lrc
luarust run hello.lrc      # the VM
luarust jit hello.lrc      # the same file, compiled to machine code
```

One implementation only ever agrees with itself. `luarust verify` runs a program two ways
and says whether they match, and `luarust fuzz` writes programs and does it in bulk — a
million of them at the last count, all compiling, all agreeing, and the fourteen thousand
that stopped part way stopping the same way both times. Built with the JIT, `fuzz` checks
all three paths on every program rather than two, and says how many of them the JIT took.

The programs it writes use everything the language has, arrays included: written out,
shaped, filled, indexed, counted, and assigned into, with indices mostly inside the array
and sometimes deliberately past the end, because a fault is an answer the three paths have
to agree on too. That last part earns its keep. Teaching the generator to write arrays
turned up a bug that had nothing to do with arrays: the compiler's constant pool deduped
with `==`, `-0` and `0` are numerically equal, and so a program that wrote `-0` and later
wrote `0` got one pool slot holding `-0` — and its `0` quietly became `-0`. The pool now
matches on representation, and there is a test for every float format.

The JIT takes every program, but it does not compile all of every program. Integers,
`b32` and `b64` become native instructions, because those are the cases where LLVM's
arithmetic and Luarust's own are both correctly rounded and so cannot differ. The rest —
`b16`, `b128`, `b256`, `bool`, `str`, and raising to a power — go back to `luarust-num` by
way of a call, and `b128`, `b256`, `bool` and `str` do not travel in registers at all: they
live in numbered cells on the Rust side and the machine code carries the number.

Cells are a **stack of frames**, one per call, which is what makes a function that calls
itself safe: each call gets its own row and nobody can overwrite the one its caller is
still holding. A celled argument travels through the runtime rather than as a machine
argument, so compiled code never has to name a cell in a frame other than its own.

That is not a compromise. Their arithmetic was always going to be a call, because none of
those formats has hardware anywhere and their answers have to come from the same place the
other two execution paths get theirs. An answer the three paths might disagree about is
worth less than no answer.

The JIT needs LLVM 21 and is behind a feature, so everything else builds with no
dependencies at all:

```bash
cargo build --release -p luarust-cli --features jit
```

**One version, and only one, on purpose.** The JIT builds cleanly against LLVM 20 as well
— changing one feature is enough, and somebody has done it and reported that all of the
tests pass, the three-way agreement suite included. It is still not supported, and the
reason is worth stating rather than leaving as a version number that looks arbitrary.

What this language promises about compiled code is that it agrees with the other two paths
bit for bit, on every program the generator can write. That promise is only worth what CI
exercises. A second LLVM version is a second code generator: it may fold a comparison
differently, it may round the same expression the same way for ten thousand programs and
not the ten-thousand-and-first, and nothing in the repository would ever find out. Shipping
it would mean making the strongest claim here about a configuration nobody tests.

So it is one version, tested, until there is a reason to run two jobs instead of one. It is
also the least costly place to be wrong: `run` and `interp` need no LLVM at all, so a
machine with the wrong version still has the whole language, minus the fastest of the ways
to run it.

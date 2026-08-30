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
does: a chain saying what kind of loop it is and what type its counter has, then a name,
then the values that set it going.

```luarust
var.local.mut.b64 ['sum'] = ['0'];

loop.range.b64 ['i'] = ['1', '100000000'] {
    set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];
}

print['sum' \n];
```

`loop.range` is what makes those two values bounds rather than two initialisers, and the
bounds are **inclusive**: that loop runs a hundred million times, one to a hundred
million, the way a range is read in mathematics.

`'i'` belongs to the loop. It exists between the braces and nowhere else, so nothing
after the loop can read the value it stopped on.

Notice there is **no semicolon after the closing brace**. A semicolon ends a statement
that finishes on a value, and a block already says where it ends. `};` would be saying
it twice.

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
bool                          true or false
str                           text
```

**There is no integer type.** Every number in Luarust is a floating-point number, and
`er` is an exact rational — a numerator over a denominator, both unbounded, so it
neither rounds nor overflows.

The binary formats are the real IEEE 754 ones rather than approximations of them:
`b16` is true half precision, and `b256` carries 237 bits of significand, which almost
nothing else on earth implements. The decimal formats are the ones where `0.1` is
exactly `0.1` and money keeps its cents.

Two of those eleven — `b32` and `b64` — are formats the hardware knows. The rest
Luarust computes itself, to the rounding the standard requires.

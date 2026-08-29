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

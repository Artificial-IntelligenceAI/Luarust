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

And when they don't share them, each name brings its own:

```luarust
var [local.str 'name', local.b16 'x', local.str '❔'] = ['Tankun', '1000', 'idk'];
```

## Scope

Every declaration carries one of three:

```luarust
var.local.str  ['name'] = ['Tankun'];
var.global.str ['name'] = ['Tankun'];
var.public.str ['name'] = ['Tankun'];
```

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

# Luarust for VS Code

Syntax highlighting and the compiler's own errors, in the Problems panel.

Covers `.lr` and `Luarust.toml`.

## What it does

**Highlighting** for `.lr`. The word lists are taken from the lexer and the parser rather
than written out by hand, and a test in the repository fails if the two drift apart.

**Errors from `luarust check`**, on save by default. It does not decide anything itself —
a second opinion written in JavaScript would be a second thing to keep correct, and it
would be wrong in ways the real one is not. What you see is what the compiler said, in the
place it said it, underlined exactly as wide as the compiler underlined it, with the rule
and the suggested fix on the hover.

**Suggestions as you type**, which follow the chain. A Luarust declaration is read left to
right — `var.local.mut.ui32` — and each dot has a small known set of things that may come
next, so the list is exactly right rather than merely long:

| after | it offers |
| --- | --- |
| `var.` | `local` `global` `public` `restricted` |
| `var.local.` | `mut`, `array`, and the nineteen types |
| `loop.` | `temp` `perm` |
| `loop.temp.` | `range` `while` |
| `fn.local.` | the types, for what it answers |
| a half-typed `'name'` | every name the file declares, including parameters |

Every type carries what it is beside it, so `d64` says *where money keeps its cents* and
`er` says *never rounds, never overflows*. There are snippets for a declaration, a
function, both loops, an `if`, a `print` and an array.

**The project file too.** `Luarust.toml` gets its own highlighting, its own suggestions
and the same errors. There are three sections and five settings, all of them enumerable,
so the editor is no vaguer about them than the compiler is:

| where | it offers |
| --- | --- |
| typing `[` | `defaults` `build` `gc` |
| in `[gc]` | `mode` |
| in `[defaults]` | `overflow`, `no-visibility-stated` — minus any already set |
| after `mode = ` | `"off"` `"silent"` `"aggressive"` |
| after `embed-source = ` | `true` `false` |

A section that is not one of the three is not coloured, which is the first sign something
is wrong — before saving, and before the compiler says so. `luarust check Luarust.toml`
does the rest:

```
2:12  [C0005] `"explode"` is not something `overflow` can be set to.
5:1   [C0001] there is no `[bulid]` section.
6:1   [C0003] `embed-source` is not under any section.
9:8   [C0005] `"sometimes"` is not something `mode` can be set to.
```

**Commands**, from the palette: check, run, run through the JIT, and show the bytecode.
The last three open a terminal so the output is yours to keep.

## Settings

| | |
| --- | --- |
| `luarust.path` | the `luarust` command; an absolute path if it is not on `PATH` |
| `luarust.checkOn` | `save` (the default), `type`, or `never` |
| `luarust.suggest` | suggestions as you type; on by default |

`type` waits for a pause rather than running on the keystroke, because a file being typed
into is a file that is half-written.

The extension turns VS Code's **word-based suggestions off** for these two languages. They
offer any word from any open file, which on top of a list that is already exactly right is
just noise — and worse than noise, because a word that looks like a setting is not one.

It also turns **inline suggestions off** for both. Copilot ships inside VS Code now, and
this language exists in one repository — there is no corpus of Luarust anywhere for a
model to have learned from. So what arrives as ghost text is a guess assembled out of Lua,
Rust and TOML, and it is wrong in the particular way that is hardest to ignore: it looks
right. `"incremental"` for `[gc] mode` is a real mode, in Lua's collector. Not one of the
three here.

The suggestion list beside it is not a guess. Its words come from the lexer and the
parser, its types from `Ty`, its settings from `luarust-conf`, and CI fails if any of them
drift. Putting a guess in front of that is a downgrade.

If you want them back — for prose in comments, or because you would rather judge for
yourself:

```json
{ "[luarust]": { "editor.inlineSuggest.enabled": true } }
```

## Installing it

Symlink it into VS Code's extensions folder and restart:

```bash
ln -s "$PWD/editors/vscode" ~/.vscode/extensions/luarust
```

Then set `luarust.path` to the built binary, since it is unlikely to be on `PATH`:

```json
{ "luarust.path": "/Users/ts/Luarust/target/release/luarust" }
```

Or press <kbd>F5</kbd> in this folder to launch a window with it loaded, which is the
better way round while changing it.

## What it is not

There is no language server, so there is no go-to-definition, no rename and no
cross-file anything. The suggestions come from the shape of the chain and from reading
the open file, not from the compiler's own idea of what is in scope — so inside a
function they will offer a name declared outside it, which the checker will then refuse.

## Keeping it honest

`check-grammar.py` compares every copied list against the compiler and fails if any of
them drifted:

- every word the parser knows must be a word the grammar colours
- the types completion offers must be exactly the types that exist
- the project file's sections and keys must be exactly what `luarust-conf` reads
- every value suggested for a setting must be one that reader accepts

`check-manifest.py` checks the other half: that the extension *starts*. Highlighting is
declarative and works whether or not the JavaScript ever runs, so a missing activation
event does not look broken — the file is coloured and the suggestions are simply absent,
with VS Code's word-based guesses filling the gap. That is a bad thing to have to notice
by eye.

It runs in CI, and each of those was checked by breaking it on purpose and watching the
check fail. A guard nobody has seen fail is not a guard.

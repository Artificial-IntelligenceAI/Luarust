# Luarust for VS Code

Syntax highlighting and the compiler's own errors, in the Problems panel.

## What it does

**Highlighting** for `.lr`. The word lists are taken from the lexer and the parser rather
than written out by hand, and a test in the repository fails if the two drift apart.

**Errors from `luarust check`**, on save by default. It does not decide anything itself —
a second opinion written in JavaScript would be a second thing to keep correct, and it
would be wrong in ways the real one is not. What you see is what the compiler said, in the
place it said it, underlined exactly as wide as the compiler underlined it, with the rule
and the suggested fix on the hover.

**Commands**, from the palette: check, run, run through the JIT, and show the bytecode.
The last three open a terminal so the output is yours to keep.

## Settings

| | |
| --- | --- |
| `luarust.path` | the `luarust` command; an absolute path if it is not on `PATH` |
| `luarust.checkOn` | `save` (the default), `type`, or `never` |

`type` waits for a pause rather than running on the keystroke, because a file being typed
into is a file that is half-written.

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

There is no language server, so there is no completion, no go-to-definition and no
rename. Everything here is the compiler being run and read.

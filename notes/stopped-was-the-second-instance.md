# `Stopped` was the second instance of the fault-boxing win

One line: the README's biggest banked win — boxing `Fault` so results fit in
registers — was applied to `Answer<T>` and never to `Stopped`, which made
`Result<Value, Stopped>` 96 bytes on the return path of every interpreter step.

Found by luarust-2b while checking my store-forwarding theory: `Answer<T>` is
`Result<T, Box<Fault>>` (16 bytes), but `Stopped` still held `Fault` inline (80
bytes), and `Stopped` is what every tree-walker `eval()` and the VM's `run()`
return. Boxing it (commit 14c87bc) took `Result<Value, Stopped>` from 96 to 32.

Measured, interleaved best-of-3 at N=1e8:

                M5 before   M5 after           x86 before   x86 after
  VM add           922        709   -23%          (see round-3 diagnose run)
  VM mod          1149       1108   -3.6%
  interp add      1800       1809   flat
  interp mod      2581       2711   +5%  <- real regression, reproducible

The M5 interp-mod regression is presumably an inlining/layout shuffle and was
unresolved when this note was written; if it survives the x86 verdict it needs
its own chase. Verified: cargo test workspace green, all three clippy configs
clean, `fuzz 200000` all agreed (8150 stopped the same way on every path).

Lesson: when a size-based win is banked, grep for the *other* types on the same
hot path with the same shape. The commit message that banked the first win
described the second bug without noticing.

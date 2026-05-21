**Never mask errors with defaults.**

> Blocking

No `unwrap_or_else(|| "default")`, no `?? <default>`, no `try?` when the error
matters, no `.ok()` dropping a Result, no `let _ = …` swallowing one, and no
silent control-flow skip — `continue`, `break`, early `return`, `return None`,
or `let Some(x) = … else { continue; };` style guards — when the guarded
condition is exceptional on the main path. Covers every language idiom that
swallows or defaults around an error: Rust `unwrap_or` / `.ok()` / `let _ = …`,
Swift `try?` / `??`, and equivalents. No carve-outs for "display defaults" or
"semantically correct nil" — empty IS the value in question, and silencing it is
the masking. If data should be present, its absence is a bug — surface it with
`expect()`, `Result`, or make the types prevent it. Masking is bad on every
axis: it hides broken assumptions and creates silent failures downstream; it
impedes structure discovery by erasing the failure cases that are part of the
system's real shape (when *can* this fail? what does it mean when it does? what
should the type encode?); and it lies about correctness — code that "works"
because errors are silenced isn't working, just quiet.

The *only* escape hatch — when a silent skip is genuinely correct behavior (the
value really is optional, the case really should be skipped) — is to log the
bail-out at the skip point: `warn!` if it's rare/abnormal, `debug!` if it's
common-but-noteworthy. This applies equally to expression-level swallowers
(`unwrap_or`, `.ok()`) and control-flow skips (`continue`, `break`, early
`return`, guard-`else`). Include the input that triggered it; "skipping CUE path
with no UTF-8 stem: {:?}" is actionable, "skipping" alone is useless. This is
the only exception. If you don't want to log, you don't have a legitimate skip —
you have a masked error. (Pure functional Option-returning helpers don't count
as bail-outs; this covers exceptional skips on the main path.)

API design follows: when a function would take `Option<T>` just to `unwrap_or`
internally, split into two — one that requires the value, a `_default()` wrapper
that generates the default and calls the first. The default stays explicit at
the boundary, not buried inside.

```rust
// Bad — Option<String> exists so internal unwrap_or can fire
pub fn create_library(name: Option<String>) -> Result<Config> {
    let name = name.unwrap_or_else(generate_library_name);
    ...
}

// Good — required param, separate default wrapper
pub fn create_library(name: String) -> Result<Config> { ... }
pub fn create_library_default() -> Result<Config> {
    create_library(generate_library_name())
}
```

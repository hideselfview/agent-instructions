---
digest: Never swallow/default around an error (unwrap_or, .ok(), try?, ??, silent continue/return); surface it, log legit skips. Redundant "belt-and-suspenders"/"just in case" backups mask the primary's failure — keep one mechanism, fail loud.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: true
---

**Never mask errors with defaults.**

No `unwrap_or_else(|| "default")`, no `?? <default>`, no `try?` when the error
matters, no `.ok()` dropping a Result, no `let _ = …` swallowing one, and no
silent control-flow skip — `continue`, `break`, early `return`, `return None`,
or `let Some(x) = … else { continue; };` style guards — when the guarded
condition is exceptional on the main path. Covers every language idiom that
swallows or defaults around an error: Rust `unwrap_or` / `.ok()` / `let _ = …`,
Swift `try?` / `??`, and equivalents. No carve-outs for read-only display
defaults or "semantically correct nil" — empty IS the value in question, and
silencing it is the masking (seeding *editable* form inputs is the one
exception, below). If data should be present, its absence is a bug — surface it
with `expect()`, `Result`, or make the types prevent it. Masking is bad on every
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

**Phrasing tells — "belt and suspenders" and friends.** When you catch yourself
describing a mechanism as "belt and suspenders," "just in case," "as a backup,"
"a safety net," "to be safe," "defensive default," or "fall back to," you are
proposing a *redundant fallback* — a second path kept alongside the
authoritative one to cover it if it fails. That is masking by another name: if
the primary silently breaks, the backup quietly covers for it and you never
learn the primary broke, so the bug lives forever behind a green build. Keep one
correct mechanism and let it fail loud; delete the backup (or, if the "primary"
genuinely can't be trusted, fix *that* — don't paper over it). These phrases are
flags everywhere they appear: code, comments, commit messages, and conversation.
Wanting two mechanisms for one guarantee is the smell; the right number is one
that works.

**Editable form-state seeding is exempt.** Converting an optional domain field
into the string state of an *editable* text input — Swift
`opt.map(String.init) ?? ""` / `opt ?? ""`, Rust `opt.unwrap_or_default()` /
`opt.map(|v| v.to_string()).unwrap_or_default()` rendering a wire field back to
raw editor text — when populating a form the user will edit is not masking. The
empty string is the input's representation of "no value", and it round-trips:
shaping/validating the form on save maps empty back to `None`/absent (the
inverse of the trim-empty-to-`None` shaping). The absence isn't a swallowed
error — it's a blank the user can fill. This is narrow to *writable* inputs:
rendering `?? "Unknown"` into read-only display to hide a missing value is still
masking.

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

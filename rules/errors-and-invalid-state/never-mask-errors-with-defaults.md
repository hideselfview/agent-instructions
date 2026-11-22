---
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
blocking: true
---

## Never mask errors with defaults

A value that should be present and isn't is a bug. Surface it with `expect()` or
a `Result`, or make the types rule the absence out. Code that works only because
its errors are silenced is not working: the silence hides the broken assumption,
moves the failure far from its cause, and erases the failure cases the types
should model.

### Swallowed errors

Never swallow or default around an error that matters, in any language:

- Expressions that drop or replace an error:
  - Rust: `unwrap_or`, `unwrap_or_else(|| "default")`, `.ok()` on a `Result`,
    `let _ = …`.
  - Swift: `try?`, `?? <default>`.
- Control flow that skips a case that is exceptional on the main path:
  - `continue`, `break`, an early `return` or `return None`.
  - A guard such as `let Some(x) = … else { continue; };`.
  - A pure helper that returns `Option` is not a skip.

Read-only display gets no exception: `?? "Unknown"` on a label hides a missing
value like any other default.

When a skip is correct (the value really is optional, or the case really should
be skipped), log it at the skip point:

- Use `warn!` when the skip is rare or abnormal, and `debug!` when it is common.
- Include the input that caused it: "skipping CUE path with no UTF-8 stem:
  {:?}", not "skipping".
- A skip you don't want to log is a masked error.

### Editable form fields

Seeding an editable text field from an optional value is not masking (Swift
`opt ?? ""`, Rust `opt.unwrap_or_default()`). The empty string is how the field
shows "no value", and saving the form maps empty back to `None`. This holds only
for fields the user can edit.

### Backup mechanisms

A second mechanism kept beside the main one, in case the main one fails, masks
errors too: when the main one breaks, the backup covers for it and nobody
learns. Keep one mechanism, let it fail loudly, and delete the backup; when the
main one can't be trusted, fix it. These words propose a backup, in code,
comments, commit messages and chat: "belt and suspenders", "just in case", "as a
backup", "a safety net", "to be safe", "defensive default", "fall back to".

A default is different: it supplies a value for a case you designed for, such as
a caller who didn't give one. Ask what makes the second path run. An absence you
planned for is a default and is allowed; the main path failing is masking and is
banned.

### Defaults at the boundary

When a function would take `Option<T>` only to `unwrap_or` inside, split it in
two: one function requires the value, and a `_default()` wrapper makes the
default and calls the first.

```rust
// Bad: the Option exists so an internal unwrap_or can fire
pub fn create_library(name: Option<String>) -> Result<Config> {
    let name = name.unwrap_or_else(generate_library_name);
    ...
}

// Good: a required parameter, and a separate default wrapper
pub fn create_library(name: String) -> Result<Config> { ... }
pub fn create_library_default() -> Result<Config> {
    create_library(generate_library_name())
}
```

### Arguments you don't know

Never pass `0`, `nil`, `None` or `""` for a parameter because it type-checks.
Trace the real value to its source. A wrong zero breaks something later: a
`samples_to_skip: 0` looks harmless and replays seconds of audio.

Empty collections are the same. `Vec::new()`, `Default::default()`, `[]` or `{}`
passed for data you don't have claims "there were none", which is a different
claim from "not captured here". When a field applies to some constructors and
not to others, make it `Option<Collection>`: `None` when the source didn't
capture it, `Some(empty)` only when the source saw none.

A parameter that really is optional is `Option` in the signature. A test may
pass defaults for parameters it doesn't exercise.

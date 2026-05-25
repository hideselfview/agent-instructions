---
digest: Don't pass 0/nil/"" because it type-checks; trace the real value or make the param Optional.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Never fill in arguments with zero-valued defaults.** Sibling to the rule
above, on the construction side. When you don't know what a parameter should be,
don't pass `0`/`nil`/`None`/`""` because it "looks safe" — trace to the real
value at the source. The default that type-checks often silently breaks
downstream (a `samples_to_skip: 0` looks harmless but causes seconds of replay
artifacts in audio playback). If the parameter genuinely is optional, the
signature should use `Option`/`Optional` so the absence is explicit. Exception:
test code can pass defaults for parameters not exercised by the test.

This covers **empty collections** too — `Vec::new()`, `IndexMap::new()`,
`Default::default()`, `[]`, `{}` — passed for data you don't have. An empty
collection asserts *"there were none"*, which is a different and usually false
claim than *"not applicable / not captured here"*. When a field is meaningful
for some constructors but inapplicable for others (e.g. a shared `HttpExchange`
whose headers an HTTP recorder captures but a browser-shim source never sees),
make it `Option<Collection>`: `None` for "not captured", `Some(empty)` only when
the source genuinely observed an empty collection. Reaching for
`Default::default()` to fill such a field because it type-checks is the same
zero-value masking as `0`/`nil`/`""` — the empty map hides that the data was
never available.

---
digest: Fix bugs at the lowest level the cause lives at — closer to the source means fewer leaks, fewer surfaces, one shape downstream.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.py'
blocking: false
---

**Fix at the lowest level the bug allows.** When you find a defect, ask where it
originates — the deserializer, the parser, the type definition, the data source
— and fix it *there*, not at the first consumer that trips over it. A
consumer-side fix protects one call site; a boundary-side fix protects every
call site, current and future, against the same input shape.

The diagnostic: if your fix is a `filter(...)`, `?? default`,
`if let Some(x), !x.is_empty()`, `try?`, or `guard let` that exists only to
launder a bad value out of a producer, the producer is the wrong shape. Move the
fix upstream until the bad value can't be produced.

**Anti-pattern — fix at the consumer.**

Discogs's JSON returns `"thumb": ""` (empty string) when a release has no cover;
serde deserializes that into `Some("")`. A consumer ten layers down trips on the
empty URL:

```rust
// In the search-result mapper, ten layers from the producer:
let cover_url = r.thumb.clone().filter(|s| !s.is_empty());
```

This works for one call site. Every other consumer of
`DiscogsSearchResult.thumb` (the release endpoint, the search endpoint, the
bridge type, the UI prefetch loop, tests) still sees `Some("")` unless they each
add the same filter. The next bug will be the same class, in a different
consumer.

**Fix at the boundary.**

```rust
// In discogs/mod.rs — shared serde helper:
pub(crate) fn empty_string_as_none<'de, D>(
    d: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(d)?.filter(|s| !s.is_empty()))
}

// On the field itself — applied once, protects every reader:
#[serde(default, deserialize_with = "crate::discogs::empty_string_as_none")]
pub thumb: Option<String>,
```

Now `thumb` is `Some("…")` or `None`, never `Some("")`, anywhere downstream. The
shape is correct at the source.

**Where "lowest" actually is.** Trace the value back through every
transformation until you hit a layer you don't control — the network, the
filesystem, the user. Fix at the highest layer you *do* control that's adjacent
to that boundary. Examples:

- Bad value in JSON from an HTTP API → custom deserializer on the field, or
  `serde(with = ...)` on the type. Not the consumer.
- Missing field that the type modeled as required → change the type to
  `Option<T>`, propagate up. Not a `.unwrap_or_default()` at the consumer.
- A function gets the wrong shape from its caller → fix the caller's call site,
  or change the function's signature so the wrong shape can't be passed. Not a
  guard in the body.
- An event arrives with the wrong fields → widen the event payload at the
  emitter. Not a "compute the missing piece by re-dereferencing other state" in
  the reducer (this is also covered by
  `reducers-must-not-read-state-to-write-state`).

**When the consumer is the right place.** Only when the consumer's context is
what makes the value bad — same value is valid elsewhere. For example, an empty
`description` field that's fine to store but shouldn't render as a tooltip —
that's a render-time decision, and the empty-vs-absent distinction belongs at
the renderer, not in the data model.

---
digest: Compose existing types/behaviors before adding a new primitive; model composites by holding existing types, not re-declaring their fields.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Compose existing primitives before adding new ones.** When a need arises, list
the primitives already in the system that touch the concern and sketch how they
combine into the answer. Adding a new primitive — a type, function, method,
keyword, endpoint, trait, config flag — when existing ones combine is a major
red flag.

When modeling a composite, hold the existing types; don't re-declare their
fields:

```rust
// Compose from existing types, not a flattened re-declaration.
struct HttpRequest {
    headers: Headers,
    body: Body,
}
```

A domain name in the new primitive's name (a specific provider, ontology,
vendor, external service) is a tell that you're baking a specific case into
vocabulary that should stay generic. This is about reusing existing
types/behaviors as building blocks — not a scalar field that copies another
field's value. *(See `principles/composable-primitives.md`. Pairs with
question-necessity and YAGNI.)*

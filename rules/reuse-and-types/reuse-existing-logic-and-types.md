---
digest: Before adding a type, function or other primitive, combine what exists; call existing logic or move it into a shared helper, merge near-copies into one parameterized unit, use the full type instead of a display copy (a copy is allowed only where a boundary stops the original), and build composites from existing types.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
blocking: false
---

## Reuse existing logic and types

Before you add a type, function, method, trait, endpoint or config flag, list
what the codebase already has for the concern and work out how those pieces
combine. Add a new one only when they can't. Two implementations of one thing
drift apart, and the one updated first leaves a bug in the other.

### Logic

- Call existing logic, or move the shared piece into a helper both callers use.
  - A push-event handler that re-derives what a reducer already computes calls
    the reducer's code.
- Flag re-implemented logic, not similar shape.
  - Two functions that call the same helpers for their shared steps, and differ
    in the rest by design, are already factored.
- Merge near-copies that one change introduces into one function or component
  with parameters.
  - Each parameter must stand for a real difference: a literal, a type, an
    expression.
  - Leave them apart when a parameter would mean nothing or would couple
    unrelated callers.
- Merge copies you find while doing other work.
  - When merging would take your change off its subject, add a work queue item
    that names every copy and where it is (`builds help lists`).
- Tests may repeat setup: setup written out in each test reads better than a
  shared fixture. Merge it only when a block is long and mechanical.

### Types

- Don't make a `FooInfo` copy of `Foo` for display: use `Foo` and ignore the
  fields you don't need.
- A copy is allowed when a boundary stops the original from crossing.
  - Such boundaries: generated bridge types (uniffi), serialization types, a
    public API kept stable, another language.
  - Test: delete the copy and use the original. If a boundary forbids that, the
    copy is required.
- A composite holds the existing types, not their fields repeated:

```rust
struct HttpRequest {
    headers: Headers,
    body: Body,
}
```

- A provider, vendor or service name in a new type's or function's name often
  means a specific case has entered vocabulary that should stay general.

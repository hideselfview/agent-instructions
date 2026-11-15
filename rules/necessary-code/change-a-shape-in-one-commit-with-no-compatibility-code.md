---
digest: When a shape changes (a table, a type that crosses a boundary, an event payload, a file layout, a wire or storage format), edit the definition and every caller in the same commit; no dual-shape flags, serde defaults that absorb renames, fallback decoders or migration shims; regenerate stale fixtures. Resetting development data is normal and never a factor in a design decision.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.sql'
blocking: false
---

## Change a shape in one commit, with no compatibility code

A shape is the definition other code reads or writes: a table, a type that
crosses a boundary (a bridge type, a serialized type), an event payload, a file
layout, a wire or storage format. When one changes, edit the definition and
update every caller in the same commit.

Old data gets no code. Flag each of these:

- a flag that switches between the old shape and the new one;
- `#[serde(default)]` added only to absorb a rename;
- a decoder that falls back to reading the old shape;
- a migration shim, or any branch that keeps reading the shape the change
  replaces.

A stale fixture is regenerated, not kept.

Resetting development data after a shape change is normal. It is never a factor
in a design decision: don't prefer an option because it avoids a reset, and
don't call a reset a cost or a caveat. A project's instructions give its reset
command, when it has one.

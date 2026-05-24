---
digest: Call existing logic or factor a shared helper; parallel implementations drift.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Don't duplicate existing logic.** Before adding code, check whether the same
logic already lives elsewhere in the repo. A push-event handler that re-derives
state already computed by a reducer should call into the existing
implementation, or factor the shared piece into a helper both call — not
parallel it. Two implementations of the same thing drift; the one updated first
becomes a silent bug in the other.

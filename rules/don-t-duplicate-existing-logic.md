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

Flag re-implemented logic, not similar shape. If two functions already call the
same helpers for their common steps and differ in the rest, the shared piece is
factored — don't flag structural parallelism between operations that diverge by
design.

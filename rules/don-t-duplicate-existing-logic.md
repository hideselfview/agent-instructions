---
digest: Call existing logic or factor a shared helper; parallel implementations drift; collapse near-copy blocks into one parameterized unit when the parameter maps to a real distinction.
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

The same goes for copies you come across while doing something else. When you
see the same check or computation written in two or more places, either merge
them into one in your current work, or, when that would pull your change off its
subject, add an item to the work queue naming every copy and where it is. Never
pass one by.

**Parameterize near-duplicates when cheap.** Two or more near-copy blocks
introduced in the same change should collapse into one function or component
with parameters when the parameterization is small, local, and the variation
maps to a real distinction (a literal, a different type, a small expression).
Skip when the abstraction would be awkward, invents a parameter without meaning,
or couples unrelated callers, but the default for obvious near-copies is to
parameterize.

This matters less in tests. Explicit per-test setup blocks read better than
fixtures hidden behind a shared helper, so duplicated test setup is acceptable;
collapse it only when a block is large and purely mechanical.

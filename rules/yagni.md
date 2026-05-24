---
digest: No dead code; delete at the root, not around it; fields that modeled the dead case become required.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**YAGNI.** Don't leave dead code around. Delete at the root, not at the consumer
— filtering or guarding around dead code preserves it. After deletion, fields
that existed only to model the dead case become required. *(See
`principles/yagni.md`. Pairs with question-necessity: YAGNI is retrospective,
question-necessity is prospective.)*

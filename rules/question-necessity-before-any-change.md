---
digest: Before any change ask 'do we need this?' — the duplicate/bug/abstraction may dissolve upstream.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Question necessity before any change.** Before any fix, refactor, or addition:
ask "do we need this?" and "what could change so we don't need this?" The
duplicate might already be handled upstream; the bug might be a symptom of a
deeper problem; the new abstraction might dissolve if the right concept is
introduced. The meta-question fires before the implementation. *(See
`principles/question-necessity.md`.)*

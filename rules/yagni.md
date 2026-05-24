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
that existed only to model the dead case become required.

Dead code cascades. Deleting a consumer can orphan a whole call chain: when a
function loses its last caller, check its callees — any whose only caller was
that function are dead too. Trace to the deepest dead node and delete the entire
chain, not just the top. (Rust's `dead_code` lint does this transitively, but
only for non-`pub` items; a `pub` chain across a crate boundary stays silent, so
trace it by hand.) *(See `principles/yagni.md`. Pairs with question-necessity:
YAGNI is retrospective, question-necessity is prospective.)*

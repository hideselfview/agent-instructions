---
digest: Model today's logic, not a future shape; never pre-shape a type or signature so tomorrow's change is smaller (no Vec for a future second item).
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Write today's shape, not tomorrow's.** Code is cheap to write and cheap to
rewrite. Don't introduce today-wrong shapes to make tomorrow's change smaller.
If today's logic always returns 0 or 1 items, the type is `Option<T>`, not
`Vec<T>` because a future change might return 2. Pre-shaping for tomorrow
introduces today's anti-patterns, and each one is its own code smell. The
downstream change widens `Option` to `Vec` when the real shape arrives; that
cost is small (search-and-replace). The opposite cost, carrying the anti-pattern
through every reader, test and review, recurs forever. The chain-PR rule "Don't
pre-scaffold for downstream chain PRs" (instructions-agent.md) is a specific
application of this principle. *(Pairs with YAGNI and question-necessity:
question-necessity is prospective at the moment of design, this rule fires when
designing for a future you don't yet have, YAGNI is retrospective on dead code
that's already there, including dead branches, never-varying params and unread
fields.)*

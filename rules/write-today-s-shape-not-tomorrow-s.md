---
digest: Model today's logic, not a future shape — no Vec-for-future, dead branches, unused fields, always-default params.
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
If today's logic always returns 0 or 1 items, the type is `Option<T>` — not
`Vec<T>` because a future change might return 2. If today's branch never fires,
delete it. If a param never varies across callers, drop it. If a field no
consumer reads, remove it. Pre-shaping for tomorrow introduces today's
anti-patterns — empty-Vec sentinels masking absence, always-default params, dead
branches, unused fields, helpers that wrap absent functionality — and each one
is its own code smell. The downstream change widens `Option` to `Vec`, adds the
param back, regrows the branch; that cost is small (search-and-replace) and the
anti-pattern eliminates itself when the real shape arrives. The opposite cost —
carrying anti-patterns through every reader, test, and review — recurs forever.
The chain-PR rule "Don't pre-scaffold for downstream chain PRs"
(instructions-agent.md) is a specific application of this principle. *(Pairs
with YAGNI and question-necessity: question-necessity is prospective at the
moment of design, this rule fires when designing for a future you don't yet
have, YAGNI is retrospective on dead code that's already there.)*

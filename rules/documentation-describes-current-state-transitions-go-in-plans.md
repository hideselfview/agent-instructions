---
digest: notes/ = timeless current state; plans/ = transitions; stale notes/ refs are deletions, not placeholders.
paths:
  - '**/*.md'
blocking: false
---

**Documentation describes current state; transitions go in `plans/`.** `notes/`
is the timeless "what is" — current code, current architecture, current data
shapes; no transient references, no aspirations. `plans/` is the "how we're
getting/got there" — migrations, design proposals, transition specs. Stale
references in `notes/` (renamed/deleted/refactored) are deletions, not
placeholders. Track `notes/` in version control; `plans/` is typically
gitignored (local working state).

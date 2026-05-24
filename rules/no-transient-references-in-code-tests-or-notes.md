---
digest: No task/plan/step codes (A7, phase 2) or temporal phrases (for now, today's bug) in committed code/tests/notes.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.md'
blocking: false
---

**No transient references in code, tests, or notes.** Don't reference the
current task, fix, plan, or session in tests, comments, docstrings, or design
notes. This includes, equally:

- **Temporal phrases** — "repro for today's bug", "fails on current code", "the
  Downloads issue", "for now", "until X lands".
- **Plan / task / step codes** — `A7`, `A9`, `B3`, `V3`, `plan 01`, `plan X1`,
  `phase 2`, `step 3`. Any letter/number tag from a planning doc. These are the
  most-missed kind: a doc comment like
  `/// Apply a user-supplied metadata edit (A7 — EditMetadataSheet) to a release`
  is a violation — `A7` is a plan step, and `(A7 — EditMetadataSheet)` will rot
  once the plan ships and the caller is renamed.
- **"will replace / will substitute / lands later" forward references** to
  unshipped work — `A2 will replace this`, `the next plan reintroduces`.

Flag every added line carrying any of these. Describe the timeless invariant
instead. Transient context belongs in commit messages and PR descriptions, not
in committed code.

---
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
  - '**/*.md'
blocking: false
---

## Committed text describes the current state

Code, comments, doc comments, tests and notes describe what is true now. How the
code got here, and where it is going, belongs in commit messages, PR
descriptions and plans.

### No references to the current work

Don't refer to the current task, fix, plan or session in committed text. Flag
each added line that holds one of these:

- A phrase tied to the present: "repro for today's bug", "fails on current
  code", "the Downloads issue", "for now", "until X lands".
- A plan, task or step code: `A7`, `B3`, `plan 01`, `phase 2`, `step 3`, or any
  letter or number tag from a planning document.
  - These are the most often missed. In
    `/// Apply a user-supplied metadata edit (A7 — EditMetadataSheet) to a release`,
    `A7` is a plan step, and the comment goes stale once the plan ships and the
    caller is renamed.
- A reference to work that hasn't shipped: "A2 will replace this", "the next
  plan reintroduces".

Write the lasting fact instead.

### Notes and plans

`notes/` describes the current code, architecture and data shapes, with no
aspirations. `plans/` describes transitions: migrations, design proposals, the
steps of a change. `notes/` is committed, and `plans/` is usually gitignored.
When a note names something renamed, deleted or rewritten, delete or rewrite
that text; never leave a placeholder.

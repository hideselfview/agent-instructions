---
name: code-rules-review
description: Review a change against the rules with `builds review`. Use to run the review early, or to settle the findings `builds land` recorded for a landing.
---

# Review a change against the rules

After `builds land` pushes, it starts a review of the landed commits against the
rules in the background, and the review never refuses or fails a landing. Use
this skill to run the same review before landing, and to settle its findings.

- Run `builds review <worktree> [<base>]` to review the worktree's commits past
  `<base>` (default `origin/main`).
  - It runs one reviewer per group of rules (a folder of `rules/` each, in
    agent-instructions) and prints the findings by group, then a total line.
  - It exits 1 when a blocking finding stands or a reviewer failed.
  - `--item N` gives the reviewers that work item's brief; `builds help` lists
    the other flags.
- Read each finding against the code and the rule clause it quotes, and fix each
  real one.
- Settle every finding with
  `builds review settle <finding> confirmed|dropped --why <text>`.
  - A dropped finding stays dropped when a rerun raises it again.
- Run `builds review` again after a fix.
  - The rerun asks only the groups that found something or failed, and tells
    them what was settled.
- Run `builds review show <landing>` to list every review run of a landing, its
  findings and how each was settled.
  - A landing's key names its items or design change, such as `items:12,13` or
    `design:4`.

Don't run one reviewer per rule yourself: `builds review` groups the rules.

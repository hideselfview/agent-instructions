---
name: code-rules-review
description: Review a change against the rules with `builds review`. Use to run the review early, or to settle the findings `builds land` recorded for a landing.
---

# Review a change against the rules

`builds land` records a review of every landing against the rules, in the
background after it pushes; it never refuses a landing. This skill is for
running that review early, and for settling its findings.

- `builds review <worktree> [<base>]` reviews the worktree's commits past
  `origin/main`. It runs one reviewer per group of rules (`review/groups.toml`
  in agent-instructions), prints the findings by group and a total line, and
  exits 1 when a blocking finding stands.
- Read each finding against the code and the rule it quotes. Fix a real one.
  Settle each with
  `builds review settle <finding> confirmed|dropped --why <text>`. A dropped
  finding stays dropped when a rerun raises it again.
- `builds review show <landing>` lists every run of a landing and how each
  finding was settled.
- Run the same command again after a fix. The rerun asks only the groups that
  found something and tells them what was settled.

Do not run one reviewer per rule yourself, and do not run a rules matrix.
`scripts/rules_review/matching_rules.py` still lists the rule files to read
while writing code.

---
name: code-rules-review
description: Review a branch against the rules with `builds review` before it lands, wait for the judge to settle each finding, fix the confirmed ones and review the fixes until no confirmed finding is open, then land. Use before every `builds land` in a repo of a project with lists, or when `builds land` refuses commits for their review.
---

# Review a branch against the rules before it lands

`builds review` never sends a reviewer the same diff twice: a review shows the
changes since the last review, and nothing else.

`builds land` refuses a branch in a repo of a project with lists while it has
unreviewed changes, a finding nobody has settled or an open confirmed finding.
Commits whose changed paths no rule group covers need no review. The agent that
did the work reviews its branch and lands it; the judge, a standing session,
settles the findings (`builds help workflow`, REVIEW). Review each branch this
way:

- Run `builds review <worktree> [<base>] --item N` as a background task, once,
  when the branch's work is committed, to review the worktree's net change past
  `<base>` (default `origin/main`).
  - It runs one reviewer per group of rules (a folder of `rules/` each, in
    agent-instructions) and prints the findings by group, then a total line.
  - `--item N` gives the reviewers that work item's brief; `builds help` lists
    the other flags.
  - It tells the judge of the findings nobody has settled, or, with no judge
    listening, your coordinator, and ends with what `builds land` would say of
    these commits. It exits 1 unless land would take them.
  - `builds review` refuses a second run of the same change while the branch's
    net change stays as the review read it. When a group gave no answer (its
    reviewer failed), a second run asks only those groups, over the same diff.
- Run `builds review wait <landing>` as a background task when the review tells
  you to. It ends once no finding waits for a settle, and prints each settle
  with its reason and each open confirmed finding.
- Fix each confirmed finding in a new commit.
- When a confirmed finding looks wrong, dispute it once instead:
  `builds review dispute <finding> --why <text>`, then `builds review wait`
  again. The judge settles it again as dropped or keeps it. When the judge keeps
  it and you still disagree, tell your coordinator; never dispute it twice.
- Run `builds review` in the branch's worktree again once the fixes are in, or
  whenever land says the branch changed since the last review (a commit added
  after the review, a squash that changed lines, or a conflict's resolution).
  - A review shows the changes since the last review, and nothing else: fix
    commits are part of them, and main's own commits that a rebase brings in are
    not. A rebase that keeps every line the branch adds and removes leaves
    nothing to read.
  - The review gives each reviewer the open confirmed findings of its rules. The
    reviewer reads the code at each one's place as it is now and closes each one
    whose problem is gone.
  - Wait for the settles of that run's findings as for the first run.
- Run `builds land` with the branch's commits and the brief's items in the
  branch's own worktree, once nothing is unreviewed and no confirmed finding is
  open.
- Run `builds review show <landing>` to list a change's review, its last review,
  each run's net change, its findings, how each was settled, and the run that
  closed each closed one.
  - A review's key defaults to the worktree's branch, such as `branch:work`.

Never settle a finding of your own branch: builds refuses it. The judge settles
each finding against the code and the rule clause it quotes, with
`builds review settle <finding> confirmed|dropped --why <text>`, and says in
`--why` what makes the finding real or wrong, since the settled findings score
the reviews (`builds review scores`). When no judge listens, the coordinator
settles them the same way.

Don't run one reviewer per rule yourself: `builds review` groups the rules.

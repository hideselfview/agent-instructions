---
name: code-rules-review
description: Review a branch against the rules with `builds review` before it lands, settle each finding, and review the fixes until no confirmed finding is open. Use before every `builds land` in a repo of a project with lists, or when `builds land` refuses commits for their review.
---

# Review a branch against the rules before it lands

`builds review` never sends a reviewer the same diff twice: a review shows the
changes since the last review, and nothing else.

`builds land` refuses a branch in a repo of a project with lists while it has
unreviewed changes or an open confirmed finding. Commits whose changed paths no
rule group covers need no review. Review each branch this way:

- Run `builds review <worktree> [<base>] --item N` once, when the branch's work
  is done, to review the worktree's net change past `<base>` (default
  `origin/main`).
  - It runs one reviewer per group of rules (a folder of `rules/` each, in
    agent-instructions) and prints the findings by group, then a total line.
  - `--item N` gives the reviewers that work item's brief; `builds help` lists
    the other flags.
  - It ends with what `builds land` would say of these commits, and exits 1
    unless land would take them.
  - `builds review` refuses a second run of the same change while the branch's
    net change stays as the review read it. When a group gave no answer (its
    reviewer failed), a second run asks only those groups, over the same diff.
- Read each finding against the code and the rule clause it quotes, and settle
  it with `builds review settle <finding> confirmed|dropped --why <text>`.
  - The settled findings are the record of what reviews catch and miss: say in
    `--why` what makes the finding real or wrong.
  - Every settle scores the reviews: a confirmed finding counts as a true
    positive and a dropped one as a false positive. `builds review scores`
    prints precision and recall per group and rule for each model and prompt,
    and `builds review replay --landing N` scores another model or prompt
    against the same settled findings. Settle on what the code does, so the
    scores show what each rule catches and what it raises wrongly.
- Have each confirmed finding fixed in a new commit.
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
  - Settle that run's findings as for the first run.
- When a confirmed finding turns out wrong, settle it again as dropped:
  `builds review settle <finding> --again --why <text>`.
- Run `builds land` with the branch's commits in its own worktree, once its
  agent is done there, nothing is unreviewed and no confirmed finding is open.
- Run `builds review show <landing>` to list a change's review, its last review,
  each run's net change, its findings, how each was settled, and the run that
  closed each closed one.
  - A review's key defaults to the worktree's branch, such as `branch:work`.

Don't run one reviewer per rule yourself: `builds review` groups the rules.

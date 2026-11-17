---
name: code-rules-review
description: Review a branch against the rules with `builds review` before it lands, settle each finding, and name the commit that fixes each confirmed one. Use before every `builds land` in a repo of a project with lists, or when `builds land` refuses commits for their review.
---

# Review a branch against the rules before it lands

No diff is reviewed more than once. Each change gets one review; the coordinator
reads the fixes for its confirmed findings.

`builds land` refuses commits in a repo of a project with lists unless a
finished review has read the branch's net change as it is now. Commits whose
changed paths no rule group covers need no review. Review each branch this way:

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
  - Read the fix yourself against its finding, and send it back when it does not
    fix the finding or breaks a rule.
  - Name it with `builds review fixed <finding> <sha>`, run in a checkout of the
    repo.
- Run `builds review` in the branch's worktree again once the fixes are in, or
  whenever land says the branch's net change differs from the one the review
  read last (a commit added after the review, a squash that changed lines, or a
  conflict's resolution).
  - As a new run of the review, it reads only how the branch's net change
    differs from the one the review's last run read, fix commits included, and
    never main's commits that a rebase brings in. A rebase that keeps every line
    the branch adds and removes leaves nothing to read.
  - Settle that run's findings and name their fixes as for the first run.
- Run `builds land` with the branch's commits in its own worktree, once its
  agent is done there.
- Run `builds review show <landing>` to list a change's review, each run's net
  change, its findings, and how each was settled and fixed.
  - A review's key defaults to the worktree's branch, such as `branch:work`.

Don't run one reviewer per rule yourself: `builds review` groups the rules.

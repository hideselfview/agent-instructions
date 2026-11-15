---
name: code-rules-review
description: Review a branch against the rules with `builds review` before it lands, settle each finding, and name the commit that fixes each confirmed one. Use before every `builds land` in a repo of a project with lists, or when `builds land` refuses commits for their review.
---

# Review a branch against the rules before it lands

`builds land` refuses commits in a repo of a project with lists unless a
finished review read exactly those commits. Commits whose changed paths no rule
group covers need no review. Review each branch this way, in one step:

- Run `builds review <worktree> [<base>] --item N` to review the worktree's
  commits past `<base>` (default `origin/main`).
  - It runs one reviewer per group of rules (a folder of `rules/` each, in
    agent-instructions) and prints the findings by group, then a total line.
  - `--item N` gives the reviewers that work item's brief; `builds help` lists
    the other flags.
  - It ends with what `builds land` would say of these commits, and exits 1
    unless land would take them.
- Read each finding against the code and the rule clause it quotes, and settle
  it with `builds review settle <finding> confirmed|dropped --why <text>`.
  - A dropped finding stays dropped when a later run raises it again.
  - The settled findings are the record of what reviews catch and miss: say in
    `--why` what makes the finding real or wrong.
- Have each confirmed finding fixed in a new commit, then name that commit with
  `builds review fixed <finding> <sha>`, run in a checkout of the repo.
- Run `builds review` again on the final commits.
  - Each group is told what was settled, and a group that already answered over
    the same commits is not asked again.
  - Settle each new finding the same way.
- Land exactly the commits the last review read.
  - Land matches them by patch id, so a rebase or cherry-pick that keeps each
    commit's change still counts; a squash or a commit added or left out does
    not.
- Run `builds review show <landing>` to list every review run of a change, the
  commits each read, its findings, and how each was settled and fixed.
  - A review's key defaults to the worktree's branch, such as `branch:work`.

Don't run one reviewer per rule yourself: `builds review` groups the rules.

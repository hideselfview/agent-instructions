---
name: code-rules-review
description: Review a branch against the rules with `builds review` before it lands, settle each finding, and name the commit that fixes each confirmed one. Use before every `builds land` in a repo of a project with lists, or when `builds land` refuses commits for their review.
---

# Review a branch against the rules before it lands

No diff is reviewed more than once. Each change gets one review; the coordinator
reads the fixes for its confirmed findings.

`builds land` refuses commits in a repo of a project with lists unless each is
one a finished review read or the named fix of one of its confirmed findings.
Commits whose changed paths no rule group covers need no review. Review each
branch this way:

- Run `builds review <worktree> [<base>] --item N` once, when the branch's work
  is done, to review the worktree's commits past `<base>` (default
  `origin/main`).
  - It runs one reviewer per group of rules (a folder of `rules/` each, in
    agent-instructions) and prints the findings by group, then a total line.
  - `--item N` gives the reviewers that work item's brief; `builds help` lists
    the other flags.
  - It ends with what `builds land` would say of these commits, and exits 1
    unless land would take them.
  - A second run of the same change is refused. The one exception: when a group
    gave no answer (its reviewer failed), a second run asks only those groups,
    over the same diff.
- Read each finding against the code and the rule clause it quotes, and settle
  it with `builds review settle <finding> confirmed|dropped --why <text>`.
  - The settled findings are the record of what reviews catch and miss: say in
    `--why` what makes the finding real or wrong.
- Have each confirmed finding fixed in a new commit.
  - builds does not review the fix: read it yourself against its finding, and
    send it back when it does not fix the finding or breaks a rule.
  - Name it with `builds review fixed <finding> <sha>`, run in a checkout of the
    repo.
- Land the reviewed commits with their named fixes.
  - Land matches commits by patch id, so a rebase or cherry-pick that keeps each
    commit's change still counts.
  - A commit added after the review, a squash, or a commit whose patch id a
    rebase changed is new work, which land refuses. Land the reviewed commits
    and their fixes without it, then review it as a change of its own. When the
    reviewed commits can't land without it, ask the person.
- Run `builds review show <landing>` to list a change's review, the commits it
  read, its findings, and how each was settled and fixed.
  - A review's key defaults to the worktree's branch, such as `branch:work`.

Don't run one reviewer per rule yourself: `builds review` groups the rules.

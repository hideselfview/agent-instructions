# Chained PRs — one branch with markers

When a stream of work ships as a chain of PRs (`#1 → #2 → … #N`, each targeting
the previous), treat the whole chain as a single linear branch. The PR-specific
branches are *markers* — labels that point at specific commits in one history,
not independent branches you maintain in parallel. This file is the operational
runbook for editing, navigating, and merging such a chain; the rules it
elaborates (`Chained PRs are one branch with markers`, `Don't cascade-rebase`,
`Don't pre-scaffold for downstream chain PRs`) live in `instructions-agent.md`.

## Editing anywhere in the chain

Edit with a single `git rebase -i main` on the chain tip: mark the target commit
`edit`, amend or add a fix commit, continue. After the rebase the chain has new
SHAs end-to-end. Walk the new commits and `git branch -f <pr-branch> <sha>` for
each PR marker, then `git push --force-with-lease origin <all-pr-branches>` in
one batched push.

### Don't cascade-rebase

Don't iterate "rebase branch 2 onto branch 1, then branch 3 onto branch 2, …".
When an upstream commit's SHA changes, downstream branches still reference the
*old* SHA in their history; git's merge-base falls back to a much older ancestor
and replays too many commits, duplicating work and producing conflicts that
don't represent real diffs. The single-rebase-on-tip model side-steps this
entirely.

## Chain navigation in PR descriptions

Each PR's description begins with a one-line nav block (bold links), then a
horizontal rule, then the PR's actual body:

```
**[Prev](https://github.com/.../pull/N-1)** | **[Next](https://github.com/.../pull/N+1)**

---

…actual PR body…
```

For the *original* chain root (the first PR opened, never had a predecessor),
drop the `[Prev]` half — the line is just `**[Next](…)**`. For the chain tip,
drop the `[Next]` half — the line is just `**[Prev](…)**`. Reviewers can walk
the chain forward or backward from any PR without leaving the diff view.

Links to merged PRs stay valid — keep `[Prev]` pointing at a merged predecessor
so post-merge readers can still walk the chain back to its context.

## Merging a chain into `main`

Always the bottom PR first (the one whose base is `main`). Locally:
`git checkout main && git merge --ff-only <bottom-pr-branch> && git push origin main`.
Delete the merged branch (`git branch -d <name>` and
`git push origin --delete <name>` — GitHub may auto-delete on merge if the repo
is configured for it). Branch deletion triggers GitHub to auto-retarget the
next-in-chain PR's base to `main`. If `main` moved beyond the merged PR while
other work landed in parallel, rebase the chain onto current `main` and
force-push the remaining markers per the edit rule above; if not, the chain tip
is already on top of `main` and no rebase runs. Leave the new head PR's `[Prev]`
link to the merged PR in place — the link still resolves and preserves
traceability. Then repeat for the next bottom PR.

See also: `instructions-agent.md` (`# Agent workflow`, `# Writing style`) for PR
shape and description rules.

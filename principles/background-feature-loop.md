# Background feature loop

How a background agent ships a non-trivial feature or fix (elaborates
"Background agents"). No PR and no waiting on CI: verify locally, merge to
`main`, and let CI on pushed `main` catch the rest.

## Invariants

- **Re-use context.** Whoever holds the context does the next step:
  - follow up with an existing agent rather than spawning one that re-reads
    everything;
  - review findings go back to the implementer that wrote the diff;
  - the planner does the reviews it can itself.
  - Stop at 75% of the context window (750k on 1M): compact, or hand off to a
    fresh agent with a written brief of state, decisions, and next step.
- **One line of work at a time** in the repo's long-lived side worktree, kept
  after each branch lands. The loop imposes no branch-per-task rule.
- **Rebase, `git merge --ff-only`, push `main` after every merge.** Fetch first
  and reconcile with origin (rebase, fix conflicts); if ff fails, rebase again.
- **Hooks run.** `--no-verify` only on the user's explicit say-so; report what
  was skipped and any known failure.

## Steps

1. **Lookover.** Read the real code paths, not summaries. Settle load-bearing
   design and correctness questions; raise genuine forks to the user.
2. **Plan** in `plans/<name>.md` — the contract an implementer builds to and a
   reviewer audits against: design, components and signatures, what to reuse,
   correctness case by case, tests per case, out of scope.
3. **Checkout.** New branch from `origin/main` in the side worktree (create it
   only if none exists). A second one only while another agent works an
   unrelated branch in the first; remove it when its work lands.
4. **Implement** in the background; the plan is the spec.
5. **One manual review** against the plan and the matching rule bodies, reading
   the actual source. No per-rule reviewers or matrix (`code-rules-review` is
   deprecated). Findings go back to the implementer; verify the fixes without a
   new sweep.
6. **Local checks: only what the change affects.**
   - Run: hooks, builds for touched platforms, tests covering the changed code
     (`cargo test -p <touched> <filter>`, one test class, one platform's lint).
     App builds run in the repo's app checkout under its lock, not in your side
     worktree (`principles/worktree-build-priming.md`).
   - Never: a whole app test suite (it launches the app on the person's screen),
     the full CI script, or other slow suites — CI runs those.
   - No pre-commit hooks in the project? Add them. Fix real failures in what you
     ran.
7. **Merge and push every time**, not in batches: rebase onto `origin/main`,
   `--ff-only`, push; re-run the fast checks if origin moved. Keep the worktree
   and move on.
8. **Batch CI fixes.** Don't block on CI; check it at pauses (between tasks, end
   of session). Fix accumulated failures together, one branch per distinct
   failure.

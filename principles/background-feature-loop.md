# Background feature loop

The concrete loop for shipping a non-trivial feature/fix through a background
agent. Elaborates the "Background agents" line in the Agent workflow section.

No PR and no waiting on GitHub CI: verification happens locally, the branch
merges straight to `main`, and CI on pushed `main` is the backstop.

Review is one manual pass by the agent holding the code and plan context.
Per-rule reviewer matrices and the `code-rules-review` workflow are deprecated;
the loop neither requires nor runs them.

## Invariants

- **Re-use context.** Whoever already holds the relevant context does the next
  step. Continue an existing agent with a follow-up message instead of spawning
  a fresh one that re-reads everything; send review findings back to the same
  implementer that produced the diff; the planner who read the code paths does
  the reviews it can do itself rather than briefing a new agent from zero.
- An agent advances one line of work at a time in one checkout. Branch
  boundaries are not an invariant of this loop. Background work runs in the
  repo's one long-lived side worktree; it is reused across every line of work
  and kept after each branch lands.
- Rebase onto current `main`, `git merge --ff-only` — no merge commits. If ff
  fails, rebase again.
- **Push `main` to origin after every merge.** Don't let local `main` accumulate
  unpushed merges — fetch first, reconcile with whatever landed on origin
  (rebase local commits onto `origin/main`, fix conflicts), then push.
- Run hooks by default. Use `--no-verify` only when the user explicitly
  authorizes skipping local verification; report what was skipped and any known
  failure before committing.

## Steps

1. **Lookover.** Read the actual code paths the change touches (not summaries).
   Settle the load-bearing design decisions and surface any correctness subtlety
   before writing anything. Raise genuine forks to the user.

2. **Plan.** Write a detailed implementation plan to `plans/<name>.md` — the
   contract. It states the design, the exact components and signatures, what to
   reuse vs not rebuild, the correctness arguments case by case, the tests that
   cover each case, and what's out of scope. Precise enough that an implementer
   implements to it and a reviewer audits against it.

3. **Checkout.** Start a branch from latest `origin/main` in the repo's side
   worktree (create it only if the repo has none). Add a second side worktree
   only when another agent is already working an unrelated branch in the first
   one, and remove that extra one when its work lands. A worktree is not a task,
   feature, branch, commit, or review boundary.

4. **Implement (background).** Dispatch an agent with the plan, in the
   background. The plan is the spec.

5. **One manual review.** Review the diff yourself against the plan and the
   applicable coding rules in one pass. Read the relevant rule bodies and actual
   source; do not dispatch reviewers per rule or run a matrix. Send confirmed
   findings back to the same implementer. Verify the corrections and read the
   relevant build/test output without restarting a review sweep.

6. **Local checks: only what the change affects; CI catches the rest.** Run the
   pre-commit hooks, the builds for the platforms the change touches, and the
   tests that cover the code it changed (e.g.
   `cargo test -p <touched> <filter>`, one test class, the one platform's lint).
   Never run a whole app test suite locally (macOS, iOS, Android) — app-hosted
   tests launch the app on the person's screen — and never the full local CI
   script or other slow suites (full-workspace test runs, release-mode tests,
   every-platform builds): those are CI's job. If the project has no pre-commit
   hooks, add them as part of the work. Fix real failures in what you did run.

7. **Merge locally, push every time — don't wait on slow verification.** When
   the fast local checks pass, fetch and rebase onto current `origin/main`,
   `git merge --ff-only`, and push `main` to origin immediately — every merge,
   not in batches. If origin moved in the meantime, reconcile (rebase onto it,
   fix conflicts, re-run the fast checks) and push. Keep the side worktree, and
   keep moving to the next task.

8. **Batch CI fix-up.** Remote CI runs on pushed `main` and catches what the
   fast local checks didn't. Don't block any loop iteration on it: let failures
   accumulate across a few merges, then fix them together in one dedicated
   CI-fix-up pass (its own single-concern branch/merge per distinct failure).
   Check CI status at natural pauses — between tasks, end of session — not after
   every push.

An agent runs its assigned line of work serially in the side worktree. The
delivery shape determines branch, commit, and review boundaries; this loop does
not impose a branch-per-task rule. Create another worktree only when another
writer must edit concurrently.

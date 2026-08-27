# Background feature loop

The concrete loop for shipping a non-trivial feature/fix through a background
agent. Elaborates the "Background agents" line in the Agent workflow section.

Two variants — **robust** and **fast**. Robust is the default; the user picks
fast when they say so ("fast", "skip CI", "merge locally"). They share the same
skeleton; fast replaces the GitHub round-trip with local verification.

## Invariants (both variants)

- **Planning runs on Fable** — or whatever the leading state-of-the-art model is
  at the time. The plan is the contract for everything downstream; never
  delegate writing it to a smaller model. Implementation can go to a smaller
  model; the plan and the reviews of work against it stay with the strongest
  one.
- **Re-use context.** Whoever already holds the relevant context does the next
  step. Continue an existing agent with a follow-up message instead of spawning
  a fresh one that re-reads everything; send review findings back to the same
  implementer that produced the diff; the planner who read the code paths does
  the reviews it can do itself rather than briefing a new agent from zero.
- An agent advances one line of work at a time in one checkout. Branch
  boundaries are not an invariant of this loop. Use a worktree only when the
  agent must edit concurrently with another writer, and reuse that worktree for
  the agent's whole line of work.
- Rebase onto current `main`, `git merge --ff-only` — no merge commits. If ff
  fails, rebase again.
- **Push `main` to origin after every merge.** Don't let local `main` accumulate
  unpushed merges in either variant — fetch first, reconcile with whatever
  landed on origin (rebase local commits onto `origin/main`, fix conflicts),
  then push.
- Run hooks by default. Use `--no-verify` only when the user explicitly
  authorizes skipping local verification; report what was skipped and any known
  failure before committing.

## Shared skeleton

1. **Lookover.** Read the actual code paths the change touches (not summaries).
   Settle the load-bearing design decisions and surface any correctness subtlety
   before writing anything. Raise genuine forks to the user.

2. **Plan.** Write a detailed implementation plan to `plans/<name>.md` — the
   contract. It states the design, the exact components and signatures, what to
   reuse vs not rebuild, the correctness arguments case by case, the tests that
   cover each case, and what's out of scope. Precise enough that
   `product-engineer` implements to it and `code-review-auditor` audits against
   it.

3. **Checkout isolation.** Give the agent a writable checkout. If it will edit
   concurrently with the root agent or another agent, create or reuse one
   worktree for it, branched from latest `main`. If there is no concurrent
   writer, the current checkout is sufficient. A worktree isolates a concurrent
   writer; it is not a task, feature, branch, commit, or review boundary.

4. **Implement (background).** Dispatch `product-engineer` with the plan, in the
   background. The plan is the spec.

5. **Audit against the plan.** When it returns, run `code-review-auditor` over
   the diff vs the plan — requirement by requirement. Fix anything
   Missing/Partial/Deviated (send findings back to the same implementer agent).
   Verify the build and tests actually pass (read the output).

Steps 6–8 diverge by variant.

## Robust variant

6. **Code rules-review.** Run the `code-rules-review` skill over the diff.
   Adjudicate each finding TP/FP; fix the true positives. Cap at 2–3 iterations
   — convergence, not perfection chasing.

7. **CI.** Push the branch and open the PR; let GitHub CI run. Fix real
   failures.

8. **Merge when green.** Rebase onto current `main`, `git merge --ff-only`, push
   `main` to origin. Clean up the worktree.

## Fast variant

No PR, no GitHub CI — verification happens locally, then the branch merges
straight to `main`.

6. **Rules review by the planner.** The planning agent reviews the diff against
   the rules itself, locally — no `code-rules-review` fan-out. It has the
   latitude to decide which rules are relevant to the diff (list candidates via
   the project's rule index / `matching_rules.py` where one exists, then judge
   relevance), then checks the diff against exactly those rules. It already
   holds the plan and the code context, so it adjudicates TP/FP in place and
   sends fixes back to the implementer.

7. **Local checks: relevant and fast only.** Run only the checks most relevant
   to the diff, and of those only the fast ones — the pre-commit hooks plus the
   targeted builds/tests/lints for the crates and platforms the change touches
   (e.g. `cargo test -p <touched>`, the one platform's lint). Do NOT run the
   full local CI script and do NOT wait on slow suites (full-workspace test
   runs, release-mode tests, every-platform builds) — those are CI's job. If the
   project has no pre-commit hooks, add them as part of the work. Fix real
   failures in what you did run.

8. **Merge locally, push every time — don't wait on slow verification.** When
   the fast local checks pass, fetch and rebase onto current `origin/main`,
   `git merge --ff-only`, and push `main` to origin immediately — every merge,
   not in batches. If origin moved in the meantime, reconcile (rebase onto it,
   fix conflicts, re-run the fast checks) and push. Clean up the worktree, and
   keep moving to the next task.

9. **Batch CI fix-up.** Remote CI runs on pushed `main` and catches what the
   fast local checks didn't. Don't block any loop iteration on it: let failures
   accumulate across a few merges, then fix them together in one dedicated
   CI-fix-up pass (its own single-concern branch/merge per distinct failure).
   Check CI status at natural pauses — between tasks, end of session — not after
   every push.

An agent runs its assigned line of work serially in the same checkout or
worktree. The delivery shape determines branch, commit, and review boundaries;
this loop does not impose a branch-per-task rule. Create another worktree only
when another writer must edit concurrently.

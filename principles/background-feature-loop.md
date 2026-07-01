# Background feature loop

The concrete loop for shipping a non-trivial feature/fix through a background
agent. Elaborates the "Background agents" line in the Agent workflow section.

1. **Lookover.** Read the actual code paths the change touches (not summaries).
   Settle the load-bearing design decisions and surface any correctness subtlety
   before writing anything. Raise genuine forks to the user.

2. **Plan.** Write a detailed implementation plan to `plans/<name>.md` — the
   contract. It states the design, the exact components and signatures, what to
   reuse vs not rebuild, the correctness arguments case by case, the tests that
   cover each case, and what's out of scope. Precise enough that
   `product-engineer` implements to it and `code-review-auditor` audits against
   it.

3. **Worktree.** Create one worktree branched from latest `main` for the agent
   to work in. One worktree per agent; one branch per feature.

4. **Implement (background).** Dispatch `product-engineer` with the plan, in the
   background. The plan is the spec.

5. **Audit against the plan.** When it returns, run `code-review-auditor` over
   the diff vs the plan — requirement by requirement. Fix anything
   Missing/Partial/ Deviated. Verify the build and tests actually pass (read the
   output).

6. **Code rules-review.** Run the `code-rules-review` skill over the diff.
   Adjudicate each finding TP/FP; fix the true positives. Cap at 2–3 iterations
   — convergence, not perfection chasing.

7. **CI.** Push the branch and open the PR; let GitHub CI run. Fix real failures
   (never `--no-verify`, never bypass a hook).

8. **Merge when green.** Rebase onto current `main`, `git merge --ff-only` (no
   merge commits). If ff fails, rebase again. Clean up the worktree.

Each task is one single-concern PR. When an agent has several tasks, it runs
them serially — one small PR per task — sharing its one worktree.

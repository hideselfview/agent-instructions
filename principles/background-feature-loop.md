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
- One worktree per agent, one branch per feature, single-concern PRs/commits.
- Rebase onto current `main`, `git merge --ff-only` — no merge commits. If ff
  fails, rebase again.
- Never `--no-verify`, never bypass a hook.

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

3. **Worktree.** Create one worktree branched from latest `main` for the agent
   to work in. One worktree per agent; one branch per feature.

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

8. **Merge when green.** Rebase onto current `main`, `git merge --ff-only`.
   Clean up the worktree.

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

7. **Local CI.** Run as much of the project's CI as exists locally instead of
   pushing to GitHub — e.g. bae's `scripts/check.sh`, plus the pre-commit hooks.
   If the project has no local check script and/or no pre-commit hooks, **add
   them as part of the work** (a `scripts/check.sh` that runs what CI runs:
   build, tests, lints, format checks) — that's the enabling investment the fast
   variant depends on, and every later loop reuses it. Fix real failures.

8. **Merge locally.** When the local checks pass, rebase onto current `main`,
   `git merge --ff-only`, clean up the worktree.

Each task is one single-concern PR (robust) or one single-concern merged branch
(fast). When an agent has several tasks, it runs them serially — one small
PR/branch per task — sharing its one worktree.

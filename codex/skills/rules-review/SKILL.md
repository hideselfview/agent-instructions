---
name: rules-review
description: Run and adjudicate agent-instructions per-rule reviews against local checkout diffs; use when a user asks to run rules review, classify findings, fix true positives, rerun targeted rules, or merge after review.
---

# Rules Review

Merge path:

1. Rebase onto current `main`.
2. Resolve merge conflicts.
3. Run rules-review against the local checkout diff.
4. Fix true positives that still apply to the current diff.
5. Rerun the relevant local rule(s), or the local full matrix when the fix
   changed the review surface.
6. Push only after local rules-review has no true positives.
7. Merge only after the branch is conflict-free and required checks pass.

Run the local Codex runner against the checkout diff. This builds review context
from `git diff <base>..HEAD` and does not fetch PR diff/context from GitHub:

```bash
python3 <agent-instructions>/scripts/rules_review/local_codex.py \
  --consumer <target-checkout> \
  --project <project> \
  --local-diff \
  --base origin/main \
  --repo <owner>/<repo> \
  --sha <head-sha> \
  --jobs 4
```

Default flow:

1. Run the local matrix without `--post`.
2. Read `SUMMARY.json`.
3. Classify each as TP or FP.
4. Fix TPs in the branch.
5. Rerun the relevant local rule(s), or the full local matrix when the fix
   changed the review surface.
6. Merge only after required checks pass and no TP remains.

Use `--slug <rule>` to rerun one discovered rule. Findings stay in
`SUMMARY.json`.

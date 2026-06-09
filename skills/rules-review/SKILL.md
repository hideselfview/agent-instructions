---
name: rules-review
description: Run and adjudicate agent-instructions per-rule PR reviews, especially local Codex rules-review matrices that post inline comments; use when a user asks to run the rules review, review AI rule comments, classify TP/FP, resolve false positives, fix true positives, rerun, or merge.
---

# Rules Review

Use the local Codex runner when Codex review should use the operator's local
Codex auth/config instead of a GitHub Actions API key:

```bash
python3 <agent-instructions>/scripts/rules_review/local_codex.py \
  --consumer <target-checkout> \
  --project <project> \
  --repo <owner>/<repo> \
  --pr-number <number> \
  --sha <head-sha> \
  --exclude '["every-bug-fix-starts-with-a-failing-test"]' \
  --jobs 4 \
  --post
```

Default flow:

1. Run the matrix and post inline comments.
2. Read every posted rules-review comment.
3. Classify each as TP or FP.
4. Reply to each comment with the decision and the reason.
5. Resolve FPs.
6. Fix TPs in the PR branch.
7. Rerun the relevant rule(s), or the full matrix when the fix changed the
   review surface.
8. Merge only after required checks pass and no TP remains.

Use `--slug <rule>` to rerun one discovered rule. Omit `--post` when testing the
runner; findings stay in `SUMMARY.json`.

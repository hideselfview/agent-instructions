---
name: code-rules-review
description: DEPRECATED. Do not run the per-rule review matrix. Review the diff yourself once against the requirements and applicable coding rules.
---

# Deprecated: per-rule code review

This skill's per-rule matrix, reviewer dispatch, model selection, and repeated
review passes are deprecated. Do not use them as a commit, PR, or merge gate.
The previous instructions to run one reviewer per rule are withdrawn.

The agent holding the implementation context performs one manual review of the
diff against the requirements and applicable coding rules. Read the relevant
source and rule bodies, fix confirmed findings, and verify those corrections
with relevant checks. Do not launch a matrix or restart an automated review
sweep.

The coding rules remain in force. `matching_rules.py` may still list relevant
rule files to read; listing them is not a review job.

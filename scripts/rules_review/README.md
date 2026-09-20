# Rule discovery and deprecated review tooling

`matching_rules.py` remains active: it lists the applicable coding-rule files
for an agent to read while authoring and reviewing a change.

The per-rule AI review workflow is deprecated. `local_codex.py` and the review
runner support in this directory are retained as historical tooling. Do not run
the matrix, dispatch reviewers per rule, or require its results before a commit,
PR, or merge.

The agent holding the implementation context reviews the diff itself once
against the requirements and applicable rules, fixes confirmed findings, and
verifies those corrections with relevant checks.

# Rule discovery

`matching_rules.py` lists the applicable coding-rule files for an agent to read
while authoring and reviewing a change.

The per-rule AI review workflow is deprecated and its tooling is removed. The
agent holding the implementation context reviews the diff itself once against
the requirements and applicable rules, fixes confirmed findings, and verifies
those corrections with relevant checks.

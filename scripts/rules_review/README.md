# Rule discovery

`matching_rules.py` lists the applicable coding-rule files for an agent to read
while authoring a change. It runs no review.

`builds review` reviews a change against the rules. Its groups of rules are the
folders of `rules/`, each with a `group.toml`. No diff is reviewed more than
once. Each change gets one review; the coordinator reads the fixes for its
confirmed findings, and `builds land` refuses commits that are neither reviewed
nor named fixes.

# Rule discovery

`matching_rules.py` lists the applicable coding-rule files for an agent to read
while authoring a change. It runs no review.

`builds review` reviews a change against the rules. Its groups of rules are in
`review/groups.toml`, and `builds land` runs it on every landing.

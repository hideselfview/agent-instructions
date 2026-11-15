# Rule discovery

`matching_rules.py` lists the applicable coding-rule files for an agent to read
while authoring a change. It runs no review.

`builds review` reviews a change against the rules. Its groups of rules are the
folders of `rules/`, each with a `group.toml`, and `builds land` records a
review of every landing.

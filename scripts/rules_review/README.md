# Rule discovery

`matching_rules.py` lists the applicable coding-rule files for an agent to read
while authoring a change. It runs no review.

`builds review` reviews a change against the rules. Its groups of rules are the
folders of `rules/`, each with a `group.toml`. No diff is reviewed more than
once. A review shows the changes since the last review, and nothing else, and
closes each open confirmed finding whose problem the code no longer has.
`builds land` refuses a branch while it has unreviewed changes or an open
confirmed finding.

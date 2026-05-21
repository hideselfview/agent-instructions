# now-doing

We're walking the release-identity PR chain (starting at bae-fm/bae#636) and,
for each PR, running the isolated-rule eval experiment against it, adjudicating
its posted findings together as true/false positives, and tuning as we go —
rewording rules that miss or over-fire, adding fixtures or samples where the
signal is noisy, and merging redundant rules — then merging each PR once we've
extracted what the experiment taught us. The eval harness lives in
`agent-instructions/eval/` (per-rule files in `rules-atomic/`, generator
`split_rules.py`); the experiment workflows are
`bae/.github/workflows/v1-*-experiment.yml`, which post findings to the PR as
`🧪`-tagged inline comments.

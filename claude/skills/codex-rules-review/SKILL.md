---
name: codex-rules-review
description: Run the local Codex per-rule rules-review matrix against a GitHub PR, then adjudicate findings (TP/FP), fix true positives, and rerun. Use when asked to run the rules review / rules matrix on a PR, check a branch against the agent-instructions rules, classify or resolve rule findings, or before merging a PR that should pass the rule set.
---

# Codex rules-review matrix

Runs one Codex reviewer **per project rule** against a GitHub PR, using the
operator's local Codex auth. Each rule gets its own codex invocation that reads
the PR diff and judges it against that single rule, emitting structured
violations.

Runner: `~/.codex/agent-instructions/scripts/rules_review/local_codex.py` (rule
discovery: `matching_rules.py` / `discover_rules.py` in the same dir).

## Prerequisites

- The change is pushed and a GitHub PR exists (the matrix reviews a PR diff, not
  a local working tree).
- `codex` CLI is installed and authed (`which codex`).
- You know the `--project` rule set: `bae` or `forage`. Cross-repo (e.g. running
  against `bae-fm/coven`) still works, but project path-scoped rules won't match
  foreign paths — you effectively exercise the **global** rules (yagni,
  dead-code, never-mask, etc.), which is usually what you want.

## Command

```bash
python3 ~/.codex/agent-instructions/scripts/rules_review/local_codex.py \
  --consumer <local checkout>        # repo on disk the line numbers map to
  --project bae \                     # rule set: bae | forage
  --repo <owner>/<repo> --pr-number <n> --sha <head-sha> \
  --jobs 4                            # rules run in parallel
```

Useful flags:

- `--list` — print discovered rule slugs and exit (no review).
- `--slug <rule>` — review only this rule; repeatable. Use to rerun the rules
  you just fixed instead of the whole matrix.
- `--exclude '["rule-a","rule-b"]'` — skip rules (commonly
  `every-bug-fix-starts-with-a-failing-test` on non-bug changes).
- `--post` — post findings as inline PR review comments. **Omit on the first
  pass**; adjudicate from `SUMMARY.json` first so you don't spam the PR with
  FPs.
- `--fail-on-findings`, `--model`, `--effort`.

## Output

The runner prints `work_dir=<temp dir>` on start. There:

- `SUMMARY.json` — every result; each violation has `path`, `line`, `blocking`,
  `body`.
- `rules/<slug>/RULES_REVIEW_OUTPUT.json` — per-rule detail.
- `PR_VIEW.json` — **the PR head the run actually reviewed** (sanity-check
  this).
- `PR_DIFF.patch` — the exact diff scored.

Read findings concisely:

```bash
cat <work_dir>/SUMMARY.json | python3 -c "
import json,sys
for r in json.load(sys.stdin)['results']:
    for v in r.get('violations',[]):
        print(f\"[{r['slug']}] {v['path']}:{v.get('line')} blocking={v.get('blocking')}\")
        print('   '+v['body'].replace(chr(10),' ')); print()
"
```

## The loop

1. `--list` to see which rules match the changed files.
2. Run **without** `--post`; read `SUMMARY.json`.
3. Classify each finding **TP / FP**. Adversarially defend (argue the strongest
   case the code is correct) before accepting a finding; keep it only if that
   fails. A finding that disagrees with a *deliberate design decision* is an FP
   — note why.
4. Fix the TPs; commit.
5. Rerun the affected rules with repeated `--slug` (or the full matrix if the
   fix moved the review surface a lot).
6. Use `--post` when you want the surviving findings as PR comments; reply to
   each with the TP/FP decision, resolve FPs, fix TPs.
7. Merge only when no TP remains (and required checks pass).

## Gotcha: the GitHub head race

The runner fetches the PR diff/view from GitHub **by PR number**, so immediately
after a push it can review the **previous** commit (GitHub hasn't registered the
new head yet). Before rerunning:

```bash
gh pr view <n> --repo <owner>/<repo> --json headRefOid -q .headRefOid
```

Confirm it equals your pushed SHA, then run. Afterward, verify
`work_dir/PR_VIEW.json` reports the SHA you expected — if it shows the old
commit, the findings are stale; rerun.

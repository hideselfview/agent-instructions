---
name: code-rules-review
description: Run the local per-rule rules-review matrix against a GitHub PR or local checkout diff, then adjudicate findings (TP/FP), fix true positives, and rerun. Use when asked to run the rules review / rules matrix on a PR, check a branch against the agent-instructions rules, classify or resolve rule findings, or before merging a PR that should pass the rule set.
---

# Code rules-review matrix

Runs one Codex reviewer **per project rule** against a GitHub PR, using the
operator's local Codex auth. Each rule gets its own codex invocation that reads
the PR diff and judges it against that single rule, emitting structured
violations.

Runner: `~/.codex/agent-instructions/scripts/rules_review/local_codex.py` (rule
discovery: `matching_rules.py` / `discover_rules.py` in the same dir).

## Prerequisites

- The change is either pushed with a GitHub PR, or available as a local checkout
  diff against a base ref.
- `codex` CLI is installed and authed (`which codex`).
- You know the `--project` rule set: `bae` or `forage`. Cross-repo (e.g. running
  against `bae-fm/coven`) still works, but project path-scoped rules won't match
  foreign paths — you effectively exercise the **global** rules (yagni,
  dead-code, never-mask, etc.), which is usually what you want.

## Commands

Preferred PR mode:

```bash
python3 ~/.codex/agent-instructions/scripts/rules_review/local_codex.py \
  --consumer <local checkout>        # repo on disk the line numbers map to
  --project bae \                     # rule set: bae | forage
  --repo <owner>/<repo> --pr-number <n> --sha <head-sha> \
  --reviewer codex \                  # backend (see Model preference below)
  --jobs 8                            # rules run in parallel; 8-10 is the sweet spot
```

Local checkout diff mode:

```bash
python3 ~/.codex/agent-instructions/scripts/rules_review/local_codex.py \
  --consumer <target-checkout> \
  --project <project> \
  --local-diff \
  --base origin/main \
  --repo <owner>/<repo> \
  --sha <head-sha> \
  --jobs 4
```

Local-diff mode builds review context from `git diff <base>..HEAD` and does not
fetch PR diff/context from GitHub. Use it when the branch should be reviewed
before a PR exists, or when GitHub's PR head is not the source of truth for the
checkout you are reviewing.

## Model preference

Best reviewer first; fall to the next only when the one above is unavailable or
out of quota:

1. **gpt-5.3-codex-spark** — codex backend, the default. A separate codex pool.
2. **gpt-5.5** — codex backend: `--reviewer codex --model gpt-5.5`. Use this
   when spark quota is exhausted — stay on codex, just switch the model.
3. **claude sonnet** — `--reviewer claude` (its default). Only when codex itself
   is unavailable.
4. **claude opus** — `--reviewer claude --model opus`. Last resort, god forbid.

Spark-out ≠ drop to claude. Spark-out → `--model gpt-5.5`. Only a dead codex CLI
sends you to claude.

**Always climb back to the top.** The fallback is per-invocation, not sticky.
Spark quota refills on a clock — the out-of-quota error names the reset time
(e.g. "try again at 4:39 PM"). So the *next* run — a rerun of errored rules, a
second pass after fixes, the next PR — starts again at **spark** (no `--model`),
not pinned to gpt-5.5. Drop to gpt-5.5 only for the rules that error *this* run,
and re-attempt spark on the next. Never stay on a lower tier just because the
previous pass fell back; the higher tier is the better reviewer and may have
recovered.

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

## The loop — hard cap on passes

**One review pass is the target. Two or three is the absolute ceiling. Never
more.** The matrix is non-deterministic and the rules overlap, so an open "fix →
rerun → fix" loop chases a moving target forever — each rerun surfaces new
phrasings of findings you've already adjudicated, burning hours and tokens for
diminishing signal. Budget the passes up front:

- **Pass 1 (almost always enough):** run the full matrix once, read
  `SUMMARY.json`, adjudicate every finding TP/FP in one sitting, fix all the TPs
  together in one commit. Stop here unless a fix was large enough to plausibly
  introduce a *new* class of violation.
- **Pass 2 (optional):** only if pass-1 fixes meaningfully reshaped the diff.
  Rerun **just the affected rules** with repeated `--slug` — not the whole
  matrix. Adjudicate and fix.
- **Pass 3 (rare, the ceiling):** only to confirm a specific TP you fixed is
  resolved. Then stop regardless of what it says.

Do not start a pass 4. If findings still surface after three passes, they are
FPs or design disagreements you've already adjudicated — record the decision and
merge. Re-running because the count is non-zero, when every remaining finding is
a known FP, is the failure mode this cap exists to prevent.

The steps within a pass:

1. `--list` to see which rules match the changed files.
2. Run **without** `--post`; read `SUMMARY.json`.
3. Classify each finding **TP / FP**. Adversarially defend (argue the strongest
   case the code is correct) before accepting a finding; keep it only if that
   fails. A finding that disagrees with a *deliberate design decision* is an FP
   — note why.
4. Fix all the TPs in one commit.
5. Merge once no *unaddressed* TP remains (and required checks pass) — a known,
   recurring FP is not a reason to rerun. `--post` is optional and only for when
   you explicitly want the surviving findings as PR comments; default is to
   adjudicate from `SUMMARY.json` and never post.

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

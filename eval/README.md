# rules-review eval

Measures whether the rules-review prompt actually catches the violations it's
supposed to catch. Each case in `cases/` is a diff plus the expected findings;
the runner replays each case through the local `claude` CLI and reports recall +
cost.

Uses the local `claude` CLI (same invocation production's
`anthropics/claude-code-action@v1` uses), so auth comes from your existing
Claude Code session — no `ANTHROPIC_API_KEY` needed. Each run costs a few
dollars; check the cost line at the end.

## Setup

```sh
cd eval
python3 -m venv venv
source venv/bin/activate
pip install -r requirements.txt
```

## Run

```sh
python runner.py             # all cases
python runner.py pr636       # only cases whose filename contains "pr636"
```

## Adding a case

Two shapes for the diff input: a saved file (preferred for PR-sized smoke tests)
or inline snippets per file.

### File-backed (PR smoke)

```yaml
# cases/pr<N>-smoke.yaml
name: "PR #<N> smoke — one-line description"
project: bae                  # which projects/<x>.md to load
diff_file: ../diffs/pr<N>.diff
expected:
  - rule: "No transient references"
    path: path/to/file.rs
```

Save the diff with `gh pr diff <N> > diffs/pr<N>.diff` and commit.

### Inline (small focused snippet)

```yaml
name: "Single-rule smoke"
project: bae
files:
  - path: path/to/file.rs
    diff: |
      @@ -1,0 +1,3 @@
      +    /// Bad doc comment with (A7) plan reference.
      +    pub fn foo() {}
expected:
  - rule: "No transient references"
    path: path/to/file.rs
```

`expected: []` means the case should produce no findings (precision test).

`rule` matches loosely against the model's `body` field — substring,
case-insensitive. Use the rule name as it appears in the rule file.

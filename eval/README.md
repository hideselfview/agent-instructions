# rules-review eval

Measures whether the rules-review prompt actually catches the violations it's
supposed to catch. Each case in `cases/` is a tiny diff with the expected
findings; the runner replays each case against the prompt and reports recall +
precision.

## Setup

```sh
cd eval
python3 -m venv venv
source venv/bin/activate
pip install -r requirements.txt
export ANTHROPIC_API_KEY=...
```

## Run

```sh
python runner.py             # all cases
python runner.py transient   # only cases whose filename contains "transient"
```

## Adding a case

`cases/<descriptive-name>.yaml`:

```yaml
name: "One-line description"
project: bae                  # which projects/<x>.md to load
files:
  - path: bae-core/src/example.rs
    diff: |
      @@ -1,0 +1,3 @@
      +    /// Bad doc comment with (A7) plan reference.
      +    pub fn foo() {}
expected:
  - rule: "No transient references"
    path: bae-core/src/example.rs
    line: 1
```

`expected: []` means the case should produce no findings (precision test).

`rule` matches loosely against the model's `body` field — substring,
case-insensitive. Match the rule name as it appears in the rule file.

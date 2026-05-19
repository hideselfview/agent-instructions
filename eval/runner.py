#!/usr/bin/env python3
"""Rules-review eval. Replays the production prompt against corpus cases."""

import os
import sys
from pathlib import Path

import yaml
from anthropic import Anthropic

REPO_ROOT = Path(__file__).resolve().parent.parent
EVAL_DIR = REPO_ROOT / "eval"
CASES_DIR = EVAL_DIR / "cases"

MODEL = "claude-opus-4-7"

REPORT_TOOL = {
    "name": "report_findings",
    "description": "Report all rule violations found in the diff. Call exactly once.",
    "input_schema": {
        "type": "object",
        "required": ["findings"],
        "properties": {
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["blocking", "path", "line", "body"],
                    "properties": {
                        "blocking": {"type": "boolean"},
                        "path": {"type": "string"},
                        "line": {"type": "integer"},
                        "body": {"type": "string"},
                    },
                },
            },
        },
    },
}


def load_rules(project: str) -> str:
    """Concatenate the rule files in the order the production prompt reads them."""
    sources = [REPO_ROOT / "instructions-code.md"]
    sources.extend(sorted((REPO_ROOT / "rules").glob("*.md")))
    project_md = REPO_ROOT / "projects" / f"{project}.md"
    if project_md.exists():
        sources.append(project_md)

    chunks = []
    for path in sources:
        rel = path.relative_to(REPO_ROOT)
        chunks.append(f"=== {rel} ===\n{path.read_text()}")
    return "\n\n".join(chunks)


def build_diff(case: dict, case_dir: Path) -> str:
    """A case is either `diff_file: path/to/file.diff` (preferred for
    PR-sized smoke tests) or `files: [{path, diff}]` (inline snippets)."""
    if "diff_file" in case:
        return (case_dir / case["diff_file"]).read_text().rstrip()
    parts = []
    for f in case["files"]:
        path = f["path"]
        parts.append(
            f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n{f['diff'].rstrip()}"
        )
    return "\n".join(parts)


def build_prompt(case: dict, rules: str, diff: str) -> str:
    return f"""Read the rule files (loaded below).

Each rule has a severity — blocking or informational. A rule is blocking
if its body contains a `> Blocking` blockquote (anywhere within the
rule's content). All other rules are informational.

Then review the diff below and flag line-anchored violations. Stay
concrete and citeable; if something is questionable but not clearly a
rule violation, skip it. Skip rules that apply to commit messages, PR
descriptions, or other non-line-anchored content.

Call the `report_findings` tool exactly once with all violations. For
each finding, populate:
- `blocking`: true if the violated rule has the `> Blocking` marker.
- `path`: file path as it appears in the diff.
- `line`: line number in the new version of the file.
- `body`: explanation, including which rule was violated (e.g.,
  "Violates **\\"Never mask errors with defaults\\"**
  (instructions-code.md): …").

If no findings, call the tool with `{{"findings": []}}`.

--- RULES ---
{rules}

--- DIFF ---
{diff}
"""


def call_claude(prompt: str) -> list:
    client = Anthropic()
    response = client.messages.create(
        model=MODEL,
        max_tokens=4096,
        tools=[REPORT_TOOL],
        tool_choice={"type": "tool", "name": "report_findings"},
        messages=[{"role": "user", "content": prompt}],
    )
    for block in response.content:
        if block.type == "tool_use" and block.name == "report_findings":
            return block.input.get("findings", [])
    raise RuntimeError(f"Model did not call report_findings. content={response.content!r}")


def match(expected: dict, finding: dict) -> bool:
    """Loose: rule name substring in body (case-insensitive), path matches."""
    body = finding.get("body", "").lower()
    if expected["rule"].lower() not in body:
        return False
    exp_path = expected.get("path")
    if exp_path and exp_path != finding.get("path"):
        return False
    return True


def compare(expected: list, found: list) -> dict:
    missed, matches = [], []
    remaining = list(found)
    for exp in expected:
        hit = next((f for f in remaining if match(exp, f)), None)
        if hit:
            matches.append((exp, hit))
            remaining.remove(hit)
        else:
            missed.append(exp)
    return {"matches": matches, "missed": missed, "unexpected": remaining}


def run_case(case_path: Path) -> dict:
    case = yaml.safe_load(case_path.read_text())
    rules = load_rules(case.get("project", "bae"))
    diff = build_diff(case, case_path.parent)
    prompt = build_prompt(case, rules, diff)
    found = call_claude(prompt)
    return {
        "name": case.get("name", case_path.stem),
        "found": found,
        **compare(case.get("expected", []), found),
    }


def main():
    filter_substr = sys.argv[1] if len(sys.argv) > 1 else ""
    cases = sorted(p for p in CASES_DIR.glob("*.yaml") if filter_substr in p.name)
    if not cases:
        print(f"No cases match '{filter_substr}'.")
        sys.exit(1)

    total_expected = 0
    total_caught = 0
    total_extra = 0
    for path in cases:
        try:
            result = run_case(path)
        except Exception as e:
            print(f"✗ {path.name}: ERROR {e}")
            continue
        n_exp = len(result["matches"]) + len(result["missed"])
        n_caught = len(result["matches"])
        n_extra = len(result["unexpected"])
        total_expected += n_exp
        total_caught += n_caught
        total_extra += n_extra
        status = "✓" if not result["missed"] and not result["unexpected"] else "✗"
        suffix = f" (+{n_extra} extra)" if n_extra else ""
        print(f"{status} {path.name}: caught {n_caught}/{n_exp}{suffix}  — {result['name']}")
        for m in result["missed"]:
            print(f"   MISSED: {m['rule']} @ {m.get('path', '?')}:{m.get('line', '?')}")
        for e in result["unexpected"]:
            print(f"   EXTRA:  {e.get('body', '')[:120]}")

    recall = total_caught / total_expected if total_expected else 0.0
    print()
    print(f"=== {total_caught}/{total_expected} expected caught ({100*recall:.0f}% recall), {total_extra} unexpected ===")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Rules-review eval. Replays the production prompt via the local
claude-code CLI (same invocation production uses, same auth).

Each case can optionally pin a historical agent-instructions commit
via `replay.agent_instructions_sha` and a specific model via
`replay.model`. Without `replay`, the case runs against the current
agent-instructions HEAD and the current production model."""

import json
import re
import subprocess
import sys
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parent.parent
EVAL_DIR = REPO_ROOT / "eval"
CASES_DIR = EVAL_DIR / "cases"

DEFAULT_MODEL = "claude-opus-4-7"
DEFAULT_ROOT_KEY = "findings"


def schema_for(root_key: str) -> dict:
    return {
        "type": "object",
        "required": [root_key],
        "properties": {
            root_key: {
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
    }


def git_show(sha: str, relpath: str) -> str:
    """Read a file at a specific commit of agent-instructions."""
    out = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "show", f"{sha}:{relpath}"],
        capture_output=True, text=True, check=True,
    )
    return out.stdout


def git_ls_tree(sha: str, dirpath: str) -> list[str]:
    """List files in a directory at a specific commit."""
    out = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "ls-tree", "--name-only", f"{sha}", f"{dirpath}/"],
        capture_output=True, text=True, check=True,
    )
    return [line for line in out.stdout.splitlines() if line.endswith(".md")]


def load_rules(project: str, sha: str | None = None) -> str:
    """Concatenate the rule files in the order the production prompt reads them.
    When `sha` is set, fetch each file at that historical commit."""
    if sha:
        paths = ["instructions-code.md"]
        paths.extend(sorted(git_ls_tree(sha, "rules")))
        project_md = f"projects/{project}.md"
        # Best-effort: skip if absent at that SHA
        try:
            git_show(sha, project_md)
            paths.append(project_md)
        except subprocess.CalledProcessError:
            pass
        chunks = [f"=== {p} ===\n{git_show(sha, p)}" for p in paths]
    else:
        sources = [REPO_ROOT / "instructions-code.md"]
        sources.extend(sorted((REPO_ROOT / "rules").glob("*.md")))
        project_md = REPO_ROOT / "projects" / f"{project}.md"
        if project_md.exists():
            sources.append(project_md)
        chunks = [f"=== {p.relative_to(REPO_ROOT)} ===\n{p.read_text()}" for p in sources]
    return "\n\n".join(chunks)


def load_prompt_template(sha: str | None) -> tuple[str, str]:
    """Extract the `prompt:` value and the json-schema root key from
    .github/workflows/rules-review.yml at the given SHA (or current).
    Returns (prompt_template, schema_root_key)."""
    if sha:
        wf_text = git_show(sha, ".github/workflows/rules-review.yml")
    else:
        wf_text = (REPO_ROOT / ".github/workflows/rules-review.yml").read_text()

    wf = yaml.safe_load(wf_text)
    # Walk steps to find the claude-code-action invocation
    steps = wf["jobs"]["review"]["steps"]
    for step in steps:
        uses = step.get("uses", "")
        if "claude-code-action" in uses:
            with_block = step.get("with", {})
            template = with_block["prompt"]
            claude_args = with_block.get("claude_args", "")
            # Extract root key from --json-schema 'JSON'
            m = re.search(r'"required":\s*\[\s*"(\w+)"\s*\]', claude_args)
            root_key = m.group(1) if m else DEFAULT_ROOT_KEY
            return template, root_key
    raise RuntimeError("Could not find claude-code-action step in workflow")


def render_template(template: str, project: str) -> str:
    """Replace the GitHub Actions template variables used by the
    production prompt. ${{ github.repository }} / pull_request.number
    don't matter for the eval (the prompt asks the model to fetch a
    diff via gh, but we inline the diff instead). `inputs.project`
    selects which projects/<x>.md to load."""
    rendered = template
    rendered = re.sub(r"\$\{\{\s*inputs\.project\s*\}\}", project, rendered)
    rendered = re.sub(r"\$\{\{\s*github\.repository\s*\}\}", "eval/case", rendered)
    rendered = re.sub(r"\$\{\{\s*github\.event\.pull_request\.number\s*\}\}", "0", rendered)
    return rendered


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


def build_prompt(prompt_template: str, project: str, rules: str, diff: str) -> str:
    """Render the production prompt template and append the inlined
    rule files and diff. The production prompt asks the model to fetch
    rules via Read and the diff via `gh pr diff $PR_NUMBER`; the eval
    side-steps that by providing both directly in the prompt body."""
    rendered = render_template(prompt_template, project)
    return f"""{rendered}

--- RULES (loaded inline by the eval runner) ---
{rules}

--- DIFF (inlined; ignore the `gh pr diff` instruction above) ---
{diff}
"""


def call_claude(prompt: str, model: str, root_key: str) -> tuple[list, float]:
    """Invoke claude-code CLI with the historical or current model and
    schema. Prompt goes via stdin — it can be 100KB+ (rules + diff).

    Returns (findings, cost_usd)."""
    proc = subprocess.run(
        [
            "claude",
            "-p",
            "--model", model,
            "--json-schema", json.dumps(schema_for(root_key)),
            "--output-format", "json",
        ],
        input=prompt,
        capture_output=True,
        text=True,
        timeout=600,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"claude exited {proc.returncode}\nstderr:\n{proc.stderr[:2000]}"
        )
    try:
        envelope = json.loads(proc.stdout)
    except json.JSONDecodeError as e:
        raise RuntimeError(f"claude stdout was not JSON: {e}\nstdout (head):\n{proc.stdout[:1500]}")

    structured = envelope.get("structured_output")
    if not isinstance(structured, dict) or root_key not in structured:
        raise RuntimeError(
            f"no structured_output.{root_key} in envelope. result={envelope.get('result', '')[:500]}"
        )
    cost = envelope.get("total_cost_usd", 0.0)
    return structured.get(root_key, []), cost


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
    project = case.get("project", "bae")
    replay = case.get("replay") or {}
    sha = replay.get("agent_instructions_sha")
    model = replay.get("model", DEFAULT_MODEL)
    rules = load_rules(project, sha)
    prompt_template, root_key = load_prompt_template(sha)
    diff = build_diff(case, case_path.parent)
    prompt = build_prompt(prompt_template, project, rules, diff)
    found, cost = call_claude(prompt, model, root_key)
    return {
        "name": case.get("name", case_path.stem),
        "found": found,
        "cost": cost,
        "model": model,
        "sha": sha or "HEAD",
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
    total_cost = 0.0
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
        total_cost += result["cost"]
        status = "✓" if not result["missed"] and not result["unexpected"] else "✗"
        suffix = f" (+{n_extra} extra)" if n_extra else ""
        print(f"{status} {path.name}: caught {n_caught}/{n_exp}{suffix}  ${result['cost']:.2f}  [{result['model']} @ {result['sha'][:9]}]  — {result['name']}")
        for m in result["missed"]:
            print(f"   MISSED: {m['rule']} @ {m.get('path', '?')}:{m.get('line', '?')}")
        for e in result["unexpected"]:
            print(f"   EXTRA:  {e.get('body', '')[:120]}")

    recall = total_caught / total_expected if total_expected else 0.0
    print()
    print(f"=== {total_caught}/{total_expected} expected caught ({100*recall:.0f}% recall), {total_extra} unexpected, ${total_cost:.2f} total ===")


if __name__ == "__main__":
    main()

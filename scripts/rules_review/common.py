#!/usr/bin/env python3
"""Shared helpers for per-rule AI review workflows."""

import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path


SCHEMA = {
    "type": "object",
    "required": ["violations"],
    "additionalProperties": False,
    "properties": {
        "violations": {
            "type": "array",
            "items": {
                "type": "object",
                "required": ["blocking", "path", "line", "body"],
                "additionalProperties": False,
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


def select_rule(rules_root, project, slug, output):
    candidates = [
        rules_root / "rules" / f"{slug}.md",
        rules_root / "projects" / f"{project}-rules" / f"{slug}.md",
    ]
    for candidate in candidates:
        if candidate.is_file():
            shutil.copyfile(candidate, output)
            return
    raise SystemExit(f"no rule file for slug: {slug}")


def fetch_pr_context(repo, pr_number, diff_out, view_out):
    with diff_out.open("w", encoding="utf-8") as f:
        subprocess.run(
            ["gh", "pr", "diff", pr_number, "--repo", repo],
            stdout=f,
            text=True,
            check=True,
        )
    with view_out.open("w", encoding="utf-8") as f:
        subprocess.run(
            [
                "gh",
                "pr",
                "view",
                pr_number,
                "--repo",
                repo,
                "--json",
                "number,title,author,baseRefName,headRefName,files,commits",
            ],
            stdout=f,
            text=True,
            check=True,
        )


def write_schema(output):
    output.write_text(json.dumps(SCHEMA, indent=2) + "\n", encoding="utf-8")


def write_prompt(repo, pr_number, output):
    output.write_text(
        f"""REPO: {repo}
PR NUMBER: {pr_number}

The ONLY rule you are enforcing is in the file ./THE_RULE.md.
Read it; that is the complete and only rule. There are no other rules.

The PR diff is in ./PR_DIFF.patch. PR metadata is in ./PR_VIEW.json.
Read those files instead of calling GitHub. Flag every line-anchored violation
of that one rule. Use repository files only when the rule needs cross-file
context. Be exhaustive.

Output structured findings as JSON matching ./RULES_REVIEW_SCHEMA.json.

For each violation, populate:
- `blocking`: the value of the `blocking:` field in the rule's frontmatter.
- `path`: file path as it appears in the diff.
- `line`: line number in the new version of the file.
- `body`: explanation of the violation.

If no violations, output {{"violations": []}}.
""",
        encoding="utf-8",
    )


def prepare(args):
    select_rule(Path(args.rules_root), args.project, args.slug, Path(args.rule_out))
    fetch_pr_context(args.repo, args.pr_number, Path(args.diff_out), Path(args.view_out))
    write_schema(Path(args.schema_out))
    write_prompt(args.repo, args.pr_number, Path(args.prompt_out))


def load_findings(path):
    if not path.is_file() or path.stat().st_size == 0:
        return None, "missing output"
    raw = path.read_text(encoding="utf-8").strip()
    if raw.startswith("```"):
        lines = raw.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].startswith("```"):
            lines = lines[:-1]
        raw = "\n".join(lines).strip()
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as e:
        return None, f"invalid JSON: {e}"
    if not isinstance(data, dict) or not isinstance(data.get("violations"), list):
        return None, "JSON does not contain a violations array"
    return data, None


def post_findings(args):
    data, error = load_findings(Path(args.findings))
    if error:
        print(f"RESULT slug={args.slug} ERROR {error}")
        return

    violations = data["violations"]
    print(f"RESULT slug={args.slug} count={len(violations)}")
    print(f"OUTJSON slug={args.slug} :: {json.dumps(data, separators=(',', ':'))}")

    for violation in violations:
        path = violation.get("path", "")
        line = violation.get("line", 0)
        payload = {
            "commit_id": args.sha,
            "path": path,
            "line": line,
            "side": "RIGHT",
            "body": f"**[rules-review: `{args.slug}`]**\n\n{violation.get('body', '')}",
        }
        result = subprocess.run(
            ["gh", "api", "-X", "POST", f"/repos/{args.repo}/pulls/{args.pr_number}/comments", "--input", "-"],
            input=json.dumps(payload),
            text=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        location = f"{path}:{line}"
        if result.returncode == 0:
            print(f"posted {args.slug} @ {location}")
        else:
            print(f"skip {args.slug} @ {location} (line not in diff?)")
            if result.stderr:
                print(result.stderr.strip(), file=sys.stderr)


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)

    schema = sub.add_parser("schema")
    schema.add_argument("--compact", action="store_true")

    prep = sub.add_parser("prepare")
    prep.add_argument("--rules-root", required=True)
    prep.add_argument("--project", required=True)
    prep.add_argument("--slug", required=True)
    prep.add_argument("--repo", required=True)
    prep.add_argument("--pr-number", required=True)
    prep.add_argument("--rule-out", default="THE_RULE.md")
    prep.add_argument("--diff-out", default="PR_DIFF.patch")
    prep.add_argument("--view-out", default="PR_VIEW.json")
    prep.add_argument("--schema-out", default="RULES_REVIEW_SCHEMA.json")
    prep.add_argument("--prompt-out", default="RULES_REVIEW_PROMPT.md")

    post = sub.add_parser("post-findings")
    post.add_argument("--findings", required=True)
    post.add_argument("--repo", required=True)
    post.add_argument("--pr-number", required=True)
    post.add_argument("--sha", required=True)
    post.add_argument("--slug", required=True)

    args = parser.parse_args()
    if args.cmd == "schema":
        print(json.dumps(SCHEMA, separators=(",", ":")) if args.compact else json.dumps(SCHEMA, indent=2))
    elif args.cmd == "prepare":
        prepare(args)
    elif args.cmd == "post-findings":
        post_findings(args)


if __name__ == "__main__":
    main()

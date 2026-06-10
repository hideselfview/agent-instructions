#!/usr/bin/env python3
"""List rule files whose path globs match the supplied repo paths.

This is for coding-time context loading. It answers: "I am about to edit these
files; which full rule bodies should I read first?"
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from discover_rules import applies, is_reviewable, parse_paths


def collect_rules(rules_root: Path, project: str | None) -> list[Path]:
    rule_files = sorted((rules_root / "rules").glob("*.md"))
    if project:
        rule_files += sorted((rules_root / "projects" / f"{project}-rules").glob("*.md"))
    return rule_files


def matching_rules(
    *,
    rules_root: Path,
    project: str | None,
    paths: list[str],
    reviewable_only: bool,
) -> list[dict[str, object]]:
    matches = []
    for rule_file in collect_rules(rules_root, project):
        text = rule_file.read_text(encoding="utf-8")
        if reviewable_only and not is_reviewable(text):
            continue
        globs = parse_paths(text)
        if applies(globs, paths):
            matches.append(
                {
                    "slug": rule_file.stem,
                    "path": str(rule_file.relative_to(rules_root)),
                    "absolute_path": str(rule_file),
                }
            )
    return matches


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rules-root", type=Path, default=Path.home() / ".codex" / "agent-instructions")
    parser.add_argument("--project")
    parser.add_argument(
        "--reviewable-only",
        action="store_true",
        help="drop rules marked review: false; coding-time loading includes them by default",
    )
    parser.add_argument("--json", action="store_true")
    parser.add_argument("paths", nargs="+", help="repo-relative paths that may be edited")
    args = parser.parse_args()

    rules_root = args.rules_root.expanduser().resolve()
    matches = matching_rules(
        rules_root=rules_root,
        project=args.project,
        paths=args.paths,
        reviewable_only=args.reviewable_only,
    )
    if args.json:
        print(json.dumps(matches, indent=2))
    else:
        for match in matches:
            print(match["absolute_path"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Discover which rules apply to a consumer repo.

A rule applies when any of its `paths` globs (frontmatter) matches a tracked
file in the consumer repo. The universe is every rule under `rules/` plus the
consumer's own `projects/<project>-rules/`. Callers may drop applicable rules
via `--exclude` (a JSON array); excluding an unknown slug is a hard error so a
rename/delete upstream surfaces in the consumer's CI instead of silently
no-op'ing.

Prints the resulting slug list as a JSON array to stdout.
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path


def frontmatter(md_text):
    """Return the YAML frontmatter block, or None if there isn't one."""
    if not md_text.startswith("---"):
        return None
    end = md_text.find("\n---", 3)
    return md_text[3:end] if end != -1 else md_text[3:]


def is_reviewable(md_text):
    """False when a rule opts out of CI review via `review: false`.

    Such a rule stays an authoring rule (loaded by its `paths`), but the
    reviewer can't verify it — e.g. it needs information it has no access to.
    """
    front = frontmatter(md_text)
    if front is None:
        return True
    m = re.search(r"^review:\s*(\S+)", front, re.MULTILINE)
    return not (m and m.group(1).strip().lower() in ("false", "no", "off"))


def parse_paths(md_text):
    """Return the list of path globs from a rule's frontmatter.

    None means no `paths:` key at all — the rule is treated as repo-wide and
    always applies. An explicit empty list is returned as [].
    """
    front = frontmatter(md_text)
    if front is None:
        return None
    lines = front.splitlines()
    for i, ln in enumerate(lines):
        inline = re.match(r"^paths:\s*\[(.*)\]\s*$", ln)
        if inline:
            return [p.strip().strip("'\"") for p in inline.group(1).split(",") if p.strip()]
        if re.match(r"^paths:\s*$", ln):
            globs = []
            for rest in lines[i + 1:]:
                item = re.match(r"^\s*-\s*(.+?)\s*$", rest)
                if item:
                    globs.append(item.group(1).strip().strip("'\""))
                elif re.match(r"^\S", rest):
                    break
            return globs
    return None


def glob_to_regex(glob):
    """Translate a gitignore-style glob to an anchored regex.

    `**/` matches zero or more directories; `*`/`?` stay within a path segment.
    """
    out, i = [], 0
    while i < len(glob):
        if glob[i:i + 3] == "**/":
            out.append("(?:.*/)?")
            i += 3
        elif glob[i:i + 2] == "**":
            out.append(".*")
            i += 2
        elif glob[i] == "*":
            out.append("[^/]*")
            i += 1
        elif glob[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(glob[i]))
            i += 1
    return re.compile("^" + "".join(out) + "$")


def applies(globs, files):
    if globs is None:
        return True
    matchers = [glob_to_regex(g) for g in globs]
    return any(m.match(f) for f in files for m in matchers)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rules-root", required=True, help="agent-instructions checkout")
    ap.add_argument("--consumer", required=True, help="consumer repo checkout")
    ap.add_argument("--project", required=True, help="agent-instructions project name")
    ap.add_argument("--exclude", default="[]", help="JSON array of slugs to drop")
    args = ap.parse_args()

    root = Path(args.rules_root)
    rule_files = sorted((root / "rules").glob("*.md"))
    proj_dir = root / "projects" / f"{args.project}-rules"
    rule_files += sorted(proj_dir.glob("*.md"))

    rules = []
    for f in rule_files:
        text = f.read_text(encoding="utf-8")
        rules.append((f.stem, parse_paths(text), is_reviewable(text)))

    exclude = json.loads(args.exclude)
    known = {slug for slug, _, _ in rules}
    unknown = [e for e in exclude if e not in known]
    if unknown:
        print(f"exclude names unknown rule(s): {unknown}", file=sys.stderr)
        sys.exit(1)

    files = subprocess.run(
        ["git", "-C", args.consumer, "ls-files"],
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()

    applicable = {
        slug for slug, paths, reviewable in rules
        if reviewable and applies(paths, files)
    }

    redundant = [e for e in exclude if e not in applicable]
    if redundant:
        print(f"note: exclude lists rule(s) not enrolled here anyway: {redundant}", file=sys.stderr)

    result = sorted(applicable - set(exclude))
    print(json.dumps(result))


if __name__ == "__main__":
    main()

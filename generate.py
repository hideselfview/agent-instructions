#!/usr/bin/env python3
"""Generate instructions.md = instructions-agent.md (always-on agent behavior)
plus a digest index built from rules/*.md frontmatter.

Each rule's full body is delivered separately by Claude Code's native path
loading (rules/ symlinked into ~/.claude/rules/); the always-on file carries
only the one-line digests so the agent knows every rule exists and can read the
full one when it's relevant. Run by install.sh.
"""

import re
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent
RULES = ROOT / "rules"
OUT = ROOT / "instructions.md"


def parse(path):
    text = path.read_text()
    m = re.match(r"^---\n(.*?)\n---\n(.*)$", text, re.DOTALL)
    if not m:
        raise SystemExit(f"{path.name}: missing frontmatter")
    front, body = m.group(1), m.group(2).strip()
    dm = re.search(r"^digest:\s*(.+)$", front, re.MULTILINE)
    if not dm:
        raise SystemExit(f"{path.name}: missing digest")
    digest = dm.group(1).strip()
    first = next((ln for ln in body.splitlines() if ln.strip()), "")
    nm = re.match(r"^\*\*(.+?)\.?\*\*", first) or re.match(r"^#+\s*(.+)$", first)
    name = nm.group(1).strip() if nm else path.stem
    return name, digest


def main():
    entries = []
    for path in sorted(RULES.glob("*.md")):
        name, digest = parse(path)
        entries.append((name, path.name, digest))

    lines = [
        "# Rules index",
        "",
        "Each rule below lives in full at `rules/<file>` and is enforced by the CI",
        "review. Its full body loads automatically when you edit a file its `paths`",
        "match; read the file directly when a rule is relevant before then.",
        "",
    ]
    for name, fname, digest in entries:
        lines.append(f"- **{name}** (`rules/{fname}`) - {digest}")

    agent = (ROOT / "instructions-agent.md").read_text().rstrip()
    OUT.write_text(agent + "\n\n" + "\n".join(lines) + "\n")
    print(f"Generated {OUT} ({len(entries)} rules indexed)")


if __name__ == "__main__":
    main()

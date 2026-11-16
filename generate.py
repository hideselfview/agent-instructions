#!/usr/bin/env python3
"""Generate tool-specific instruction files from shared agent behavior.

Claude Code gets only instructions-agent.md because it loads path-scoped rules
natively from rules/ symlinked into ~/.claude/rules/. Codex gets
instructions-agent.md plus a digest index built from the frontmatter of the rules under rules/
because it does not have native path-scoped rule loading. Run by install.sh.
"""

import re
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent
RULES = ROOT / "rules"
CLAUDE_OUT = ROOT / "claude" / "CLAUDE.md"
CODEX_OUT = ROOT / "codex" / "AGENTS.md"


def parse(path):
    text = path.read_text()
    m = re.match(r"^---\n(.*?)\n---\n(.*)$", text, re.DOTALL)
    if not m:
        raise SystemExit(f"{path.name}: missing frontmatter")
    front, body = m.group(1), m.group(2).strip()
    # A digest is one line, or a folded block (`digest: >-`) of indented lines.
    dm = re.search(r"^digest:[ \t]*(?:>-?[ \t]*\n((?:[ \t]+.*\n?)+)|(.+))$", front, re.MULTILINE)
    if not dm:
        raise SystemExit(f"{path.name}: missing digest")
    if dm.group(1) is not None:
        digest = " ".join(line.strip() for line in dm.group(1).splitlines() if line.strip())
    else:
        digest = dm.group(2).strip()
    first = next((ln for ln in body.splitlines() if ln.strip()), "")
    nm = re.match(r"^\*\*(.+?)\.?\*\*", first) or re.match(r"^#+\s*(.+)$", first)
    name = nm.group(1).strip() if nm else path.stem
    return name, digest


def main():
    entries = []
    for path in sorted(RULES.rglob("*.md")):
        name, digest = parse(path)
        entries.append((name, path.relative_to(ROOT), digest))

    lines = [
        "# Rules index",
        "",
        "Each rule below lives in full at the path given.",
        "",
        "Claude Code loads matching rule bodies automatically from",
        "`~/.claude/rules/`.",
        "",
        "Codex does not have Claude's path-scoped rule autoload. Before editing",
        "files, list the matching full rule bodies and read them completely:",
        "",
        "```sh",
        "python3 ~/.codex/agent-instructions/scripts/rules_review/matching_rules.py \\",
        "  --project <project-name> <repo-relative-path>...",
        "```",
        "",
        "If no project-specific rules apply, omit `--project`. Use the project",
        "name from the project doc, e.g. `bae` for `projects/bae.md`.",
        "",
    ]
    for name, fname, digest in entries:
        lines.append(f"- **{name}** (`{fname}`) - {digest}")

    agent = (
        (ROOT / "instructions-agent.md")
        .read_text()
        .rstrip()
        .replace("{{agent-instructions}}", str(ROOT))
    )
    CLAUDE_OUT.parent.mkdir(parents=True, exist_ok=True)
    CODEX_OUT.parent.mkdir(parents=True, exist_ok=True)
    CLAUDE_OUT.write_text(agent + "\n")
    CODEX_OUT.write_text(agent + "\n\n" + "\n".join(lines) + "\n")
    print(f"Generated {CLAUDE_OUT}")
    print(f"Generated {CODEX_OUT} ({len(entries)} rules indexed)")


if __name__ == "__main__":
    main()

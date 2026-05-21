#!/usr/bin/env python3
"""Split the canonical rule files into one file per rule under
eval/rules-atomic/<slug>.md, so experiments can load a single rule's
exact text by filename instead of extracting blocks at runtime.

Re-run whenever the rules change:  python eval/split_rules.py
"""

import re
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "eval" / "rules-atomic"

# (file, style) — "bold" = instructions-code.md `**Name.**` paragraphs;
# "h2" = `## Name` sections in rules/*.md and projects/*.md.
SOURCES = [
    ("instructions-code.md", "bold"),
    ("rules/rust-patterns.md", "h2"),
    ("rules/swift.md", "h2"),
    ("rules/reactive-ui.md", "h2"),
    ("projects/bae.md", "h2"),
]


def slugify(heading: str) -> str:
    h = heading.strip().rstrip(".")
    h = h.replace("`", "").replace("→", " ")
    h = re.sub(r"[^a-zA-Z0-9]+", "-", h).strip("-").lower()
    return h


def split_bold(text: str):
    lines = text.splitlines()
    idxs = [i for i, ln in enumerate(lines) if re.match(r"^\*\*[A-Z]", ln)]
    for k, start in enumerate(idxs):
        end = idxs[k + 1] if k + 1 < len(idxs) else len(lines)
        block = "\n".join(lines[start:end]).strip()
        m = re.match(r"^\*\*(.+?)\.?\*\*", lines[start])
        yield m.group(1), block


def split_h2(text: str):
    lines = text.splitlines()
    idxs = [i for i, ln in enumerate(lines) if ln.startswith("## ")]
    for k, start in enumerate(idxs):
        end = idxs[k + 1] if k + 1 < len(idxs) else len(lines)
        block = "\n".join(lines[start:end]).strip()
        yield lines[start][3:].strip(), block


def main():
    if OUT.exists():
        for f in OUT.glob("*.md"):
            f.unlink()
    OUT.mkdir(parents=True, exist_ok=True)
    slugs = []
    for relpath, style in SOURCES:
        text = (ROOT / relpath).read_text()
        splitter = split_bold if style == "bold" else split_h2
        for heading, block in splitter(text):
            slug = slugify(heading)
            (OUT / f"{slug}.md").write_text(block + "\n")
            slugs.append((slug, relpath, heading))
    print(f"wrote {len(slugs)} rule files to {OUT.relative_to(ROOT)}")
    for slug, relpath, heading in slugs:
        print(f"  {slug:42s} {relpath}")


if __name__ == "__main__":
    main()

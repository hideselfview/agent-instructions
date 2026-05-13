#!/usr/bin/env bash
# Symlink agent-instructions content into ~/.claude/ so Claude Code reads
# this repo's files as its user-level CLAUDE.md, rules, and principles.
#
# Idempotent — safe to re-run after pulling new files.

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ln -sf "$repo/CLAUDE.md" "$HOME/.claude/CLAUDE.md"
echo "Linked ~/.claude/CLAUDE.md -> $repo/CLAUDE.md"

for subdir in rules principles; do
  mkdir -p "$HOME/.claude/$subdir"
  for f in "$repo/$subdir"/*.md; do
    [[ -f "$f" ]] || continue
    name="$(basename "$f")"
    ln -sf "$f" "$HOME/.claude/$subdir/$name"
    echo "Linked ~/.claude/$subdir/$name -> $f"
  done
done

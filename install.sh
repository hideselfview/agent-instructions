#!/usr/bin/env bash
# Symlink agent-instructions content into ~/.claude/ so Claude Code reads
# this repo's files as its user-level CLAUDE.md and rules.
#
# Idempotent — safe to re-run after pulling new rule files.

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

mkdir -p "$HOME/.claude/rules"

ln -sf "$repo/CLAUDE.md" "$HOME/.claude/CLAUDE.md"
echo "Linked ~/.claude/CLAUDE.md -> $repo/CLAUDE.md"

for rule in "$repo"/rules/*.md; do
  name="$(basename "$rule")"
  ln -sf "$rule" "$HOME/.claude/rules/$name"
  echo "Linked ~/.claude/rules/$name -> $rule"
done

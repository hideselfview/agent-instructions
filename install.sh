#!/usr/bin/env bash
# Symlink agent-instructions content into ~/.claude/ so Claude Code reads
# this repo's files as its user-level CLAUDE.md, rules, principles, and
# agents.
#
# Idempotent — safe to re-run after pulling new files.

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ln -sf "$repo/CLAUDE.md" "$HOME/.claude/CLAUDE.md"
echo "Linked ~/.claude/CLAUDE.md -> $repo/CLAUDE.md"

ln -sf "$repo/settings.json" "$HOME/.claude/settings.json"
echo "Linked ~/.claude/settings.json -> $repo/settings.json"

for subdir in rules principles agents; do
  mkdir -p "$HOME/.claude/$subdir"
  for f in "$repo/$subdir"/*.md; do
    [[ -f "$f" ]] || continue
    name="$(basename "$f")"
    ln -sf "$f" "$HOME/.claude/$subdir/$name"
    echo "Linked ~/.claude/$subdir/$name -> $f"
  done
done

# Per-project CLAUDE.md symlinks. Convention: each projects/<name>.md
# targets ~/dev/<name>/CLAUDE.md. Skip projects whose target dir doesn't
# exist on this machine.
for f in "$repo/projects"/*.md; do
  [[ -f "$f" ]] || continue
  name="$(basename "$f" .md)"
  target_dir="$HOME/dev/$name"
  if [[ -d "$target_dir" ]]; then
    ln -sf "$f" "$target_dir/CLAUDE.md"
    echo "Linked $target_dir/CLAUDE.md -> $f"
  else
    echo "Skipped $name (no $target_dir)"
  fi
done

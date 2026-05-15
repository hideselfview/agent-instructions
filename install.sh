#!/usr/bin/env bash
# Symlink agent-instructions content into Claude Code and Codex homes so both
# tools read the same user-level instructions. Also installs the markdown
# pre-commit hook for this repo.
#
# Idempotent — safe to re-run after pulling new files.

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Pre-commit hook: enforce mdformat on markdown files.
if ! command -v mdformat >/dev/null 2>&1; then
  echo "Installing mdformat via pipx..."
  command -v pipx >/dev/null 2>&1 || brew install pipx
  pipx install mdformat
fi
if ! mdformat --version 2>/dev/null | grep -q mdformat_frontmatter; then
  echo "Injecting mdformat-frontmatter plugin..."
  pipx inject mdformat mdformat-frontmatter
fi
git -C "$repo" config core.hooksPath .githooks
echo "Configured $repo git hooks -> .githooks"

mkdir -p "$HOME/.claude" "$HOME/.codex"

ln -sf "$repo/instructions.md" "$HOME/.claude/CLAUDE.md"
echo "Linked ~/.claude/CLAUDE.md -> $repo/instructions.md"

ln -sf "$repo/instructions.md" "$HOME/.codex/AGENTS.md"
echo "Linked ~/.codex/AGENTS.md -> $repo/instructions.md"

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

# Per-project instruction symlinks. Convention: each projects/<name>.md
# targets ~/dev/<name>/CLAUDE.md and ~/dev/<name>/AGENTS.md. Skip projects whose
# target dir doesn't exist on this machine.
for f in "$repo/projects"/*.md; do
  [[ -f "$f" ]] || continue
  name="$(basename "$f" .md)"
  target_dir="$HOME/dev/$name"
  if [[ -d "$target_dir" ]]; then
    ln -sf "$f" "$target_dir/CLAUDE.md"
    echo "Linked $target_dir/CLAUDE.md -> $f"
    ln -sf "$f" "$target_dir/AGENTS.md"
    echo "Linked $target_dir/AGENTS.md -> $f"
  else
    echo "Skipped $name (no $target_dir)"
  fi
done

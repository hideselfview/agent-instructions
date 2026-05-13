#!/usr/bin/env bash
# Symlink a project-specific CLAUDE.md from agent-instructions/projects/
# into a project working tree.
#
# Usage: link-project.sh <project-name> <target-dir>
# Example: link-project.sh forage ~/dev/forage
#
# Run once per new checkout/worktree. Idempotent.

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $(basename "$0") <project-name> <target-dir>" >&2
  exit 2
fi

project="$1"
target_dir="$2"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$repo/projects/$project.md"

if [[ ! -f "$src" ]]; then
  echo "error: $src does not exist" >&2
  exit 1
fi

if [[ ! -d "$target_dir" ]]; then
  echo "error: $target_dir is not a directory" >&2
  exit 1
fi

ln -sf "$src" "$target_dir/CLAUDE.md"
echo "Linked $target_dir/CLAUDE.md -> $src"

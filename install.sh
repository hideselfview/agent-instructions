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

# Generate gitignored tool-specific instruction files. Claude gets shared
# always-on behavior; Codex gets that plus the rules index it needs because it
# lacks Claude Code's path-scoped rule loading. Rule bodies are delivered
# separately by Claude Code's rules/ symlinks and by Codex's matching_rules.py.
python3 "$repo/generate.py"
mdformat "$repo/claude/CLAUDE.md" "$repo/codex/AGENTS.md"

ln -sf "claude/CLAUDE.md" "$repo/CLAUDE.md"
ln -sf "codex/AGENTS.md" "$repo/AGENTS.md"

mkdir -p "$HOME/.claude" "$HOME/.codex"

ln -sf "$repo/claude/CLAUDE.md" "$HOME/.claude/CLAUDE.md"
echo "Linked ~/.claude/CLAUDE.md -> $repo/claude/CLAUDE.md"

ln -sf "$repo/codex/AGENTS.md" "$HOME/.codex/AGENTS.md"
echo "Linked ~/.codex/AGENTS.md -> $repo/codex/AGENTS.md"

ln -sfn "$repo" "$HOME/.codex/agent-instructions"
echo "Linked ~/.codex/agent-instructions -> $repo"

codex_config_target="$HOME/.codex/config.toml"
if [[ -e "$codex_config_target" && ! -L "$codex_config_target" ]]; then
  codex_config_backup="$HOME/.codex/config.toml.local.$(date +%Y%m%d%H%M%S)"
  mv "$codex_config_target" "$codex_config_backup"
  echo "Moved existing ~/.codex/config.toml -> $codex_config_backup"
fi
ln -sf "$repo/codex/config.toml" "$codex_config_target"
echo "Linked ~/.codex/config.toml -> $repo/codex/config.toml"

ln -sf "$repo/claude/settings.json" "$HOME/.claude/settings.json"
echo "Linked ~/.claude/settings.json -> $repo/claude/settings.json"

if [[ -d "$repo/codex/skills" ]]; then
  mkdir -p "$HOME/.codex/skills"
  for stale_link in "$HOME/.codex/skills"/*; do
    [[ -L "$stale_link" && ! -e "$stale_link" ]] || continue
    rm "$stale_link"
  done
  for skill_dir in "$repo/codex/skills"/*; do
    [[ -d "$skill_dir" ]] || continue
    target="$HOME/.codex/skills/$(basename "$skill_dir")"
    if [[ -e "$target" && ! -L "$target" ]]; then
      echo "Skipped $target (exists and is not a symlink)"
      continue
    fi
    ln -sfn "$skill_dir" "$target"
    echo "Linked $target -> $skill_dir"
  done
fi

if [[ -d "$repo/claude/skills" ]]; then
  mkdir -p "$HOME/.claude/skills"
  for stale_link in "$HOME/.claude/skills"/*; do
    [[ -L "$stale_link" && ! -e "$stale_link" ]] || continue
    rm "$stale_link"
  done
  for skill_dir in "$repo/claude/skills"/*; do
    [[ -d "$skill_dir" ]] || continue
    target="$HOME/.claude/skills/$(basename "$skill_dir")"
    if [[ -e "$target" && ! -L "$target" ]]; then
      echo "Skipped $target (exists and is not a symlink)"
      continue
    fi
    ln -sfn "$skill_dir" "$target"
    echo "Linked $target -> $skill_dir"
  done
fi

for subdir in rules principles; do
  mkdir -p "$HOME/.claude/$subdir"
  for f in "$repo/$subdir"/*.md; do
    [[ -f "$f" ]] || continue
    name="$(basename "$f")"
    ln -sf "$f" "$HOME/.claude/$subdir/$name"
    echo "Linked ~/.claude/$subdir/$name -> $f"
  done
done

if [[ -d "$repo/claude/agents" ]]; then
  mkdir -p "$HOME/.claude/agents"
  for f in "$repo/claude/agents"/*.md; do
    [[ -f "$f" ]] || continue
    name="$(basename "$f")"
    ln -sf "$f" "$HOME/.claude/agents/$name"
    echo "Linked ~/.claude/agents/$name -> $f"
  done
fi

# Disk-reclaim launchd agent: sweep regenerable build artifacts/caches every 15
# minutes, but only delete when free space is below the threshold (most runs are
# no-ops). The script is tool-agnostic; this timer is what makes it automatic.
reclaim_script="$repo/scripts/reclaim-disk/reclaim.sh"
if [[ -f "$reclaim_script" ]]; then
  chmod +x "$reclaim_script"
  la_dir="$HOME/Library/LaunchAgents"
  mkdir -p "$la_dir" "$HOME/Library/Logs"
  plist="$la_dir/com.dima.reclaim-disk.plist"
  cat >"$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.dima.reclaim-disk</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>$reclaim_script</string>
  </array>
  <key>StartInterval</key><integer>900</integer>
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>$HOME/Library/Logs/reclaim-disk.out.log</string>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/reclaim-disk.err.log</string>
</dict>
</plist>
PLIST
  launchctl bootout "gui/$(id -u)/com.dima.reclaim-disk" 2>/dev/null || true
  if launchctl bootstrap "gui/$(id -u)" "$plist" 2>/dev/null; then
    echo "Loaded disk-reclaim launchd agent (every 15 min) -> $plist"
  else
    echo "Wrote $plist (run: launchctl bootstrap gui/\$(id -u) \"$plist\")"
  fi
fi

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

# Per-project rule files. Convention: projects/<name>-rules/*.md are path-scoped
# rules for that project, symlinked into ~/dev/<name>/.claude/rules/ so Claude
# Code loads them project-scoped. Gitignore .claude/rules/ in the target repo.
for rules_dir in "$repo/projects"/*-rules; do
  [[ -d "$rules_dir" ]] || continue
  name="$(basename "$rules_dir")"
  name="${name%-rules}"
  if [[ -d "$HOME/dev/$name" ]]; then
    target_dir="$HOME/dev/$name/.claude/rules"
    mkdir -p "$target_dir"
    for f in "$rules_dir"/*.md; do
      [[ -f "$f" ]] || continue
      ln -sf "$f" "$target_dir/$(basename "$f")"
      echo "Linked $target_dir/$(basename "$f") -> $f"
    done
  else
    echo "Skipped ${name}-rules (no $HOME/dev/$name)"
  fi
done

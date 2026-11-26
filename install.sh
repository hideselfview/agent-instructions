#!/usr/bin/env bash
# Symlink agent-instructions content into Claude Code and Codex homes so both
# tools read the same user-level instructions. Also points git at the repo's
# hooks (.githooks, whose pre-commit runs `builds checks run`), installs
# mdformat, which those checks run, and builds and installs the `builds` tool.
#
# Idempotent — safe to re-run after pulling new files.

set -euo pipefail

# The main checkout, whichever worktree this runs from: the live config always
# points there, so nothing in a side worktree goes live before it lands.
repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" worktree list --porcelain | sed -n '1s/^worktree //p')"

# How this install ends goes into the repo's git folder, where `builds land`
# and `builds health` read it: `ok <commit>`, or `failed <commit> <step>` for
# the step that was running when the script stopped. The post-merge hook runs
# this script, and git does not report a hook's failure, so without the record
# nothing says that the builds on PATH is older than main. The record is
# written by this script, not by builds: an install that fails before the
# builds program is built has no program to write it.
commit="$(git -C "$repo" rev-parse HEAD)"
record="$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)/builds-install"
step="starting"
record_end() {
  local status=$?
  [[ $status -eq 0 ]] || echo "failed $commit $step" > "$record"
}
trap record_end EXIT

step="installing mdformat"
# mdformat, which the commit checks run on markdown (.checks).
if ! command -v mdformat >/dev/null 2>&1; then
  echo "Installing mdformat via pipx..."
  command -v pipx >/dev/null 2>&1 || brew install pipx
  pipx install mdformat
fi
if ! mdformat --version 2>/dev/null | grep -q mdformat_frontmatter; then
  echo "Injecting mdformat-frontmatter plugin..."
  pipx inject mdformat mdformat-frontmatter
fi
step="linking instructions and skills"
git -C "$repo" config core.hooksPath .githooks
echo "Configured $repo git hooks -> .githooks"

mkdir -p "$HOME/.claude" "$HOME/.codex"

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

# The build volume's tool, a Rust program: built here, then its own `install`
# copies it onto PATH in one rename, so a failed build leaves the installed one
# as it was, and sets up the shared caches and the sccache server. It is built
# from the committed HEAD, never the working tree: another session's commit
# runs this too, and would otherwise install whatever edit is in progress in
# main. The export is synced into a folder that stays put, rewriting only the
# files that changed, so cargo rebuilds only when the committed source did.
# builds depends on the shots checker and the boards library by their paths,
# ../shots and ../boards, so the libraries are synced beside it.
step="syncing the builds sources"
head="$repo/scripts/builds/target/head"
rm -rf "$head/export"
mkdir -p "$head/export" "$head/src" "$head/boards" "$head/shots"
git -C "$repo" archive HEAD scripts/builds scripts/boards scripts/shots | tar -x -C "$head/export"
rsync -rc --delete "$head/export/scripts/builds/" "$head/src/"
rsync -rc --delete "$head/export/scripts/boards/" "$head/boards/"
rsync -rc --delete "$head/export/scripts/shots/" "$head/shots/"
rm -rf "$head/export"
step="building builds with cargo"
cargo build --release --quiet --locked --manifest-path "$head/src/Cargo.toml" --target-dir "$head/target"
step="running builds install"
"$head/target/release/builds" install
step="linking the instructions files and rules"

for file in "$HOME/.claude/CLAUDE.md" "$HOME/.codex/AGENTS.md"; do
  ln -sf "$repo/instructions-agent.md" "$file"
  echo "Linked $file -> $repo/instructions-agent.md"
done
rm -f "$repo/claude/CLAUDE.md" "$repo/codex/AGENTS.md"

# Remove links installed by the old rule loader. Leave other rules in place.
remove_rule_links() {
  local path target
  for path in "$1"/*.md; do
    [[ -L "$path" ]] || continue
    target="$(readlink "$path")"
    [[ "$target" == "$2"/* ]] || continue
    rm "$path"
  done
}

remove_rule_links "$HOME/.claude/rules" "$repo/rules"

for rules_dir in "$repo/projects/"*-rules; do
  [[ -d "$rules_dir" ]] || continue
  name="${rules_dir##*/}"
  project="$HOME/dev/${name%-rules}"
  [[ -e "$project/.git" ]] || continue
  checkouts="$(git -C "$project" worktree list --porcelain | sed -n 's/^worktree //p')"
  while IFS= read -r checkout; do
    remove_rule_links "$checkout/.claude/rules" "$rules_dir"
  done <<< "$checkouts"
done

echo "ok $commit" > "$record"

#!/usr/bin/env bash
# Reclaim disk space by deleting regenerable build artifacts and caches, in
# priority order, until free space climbs back above a target. Built to run
# unattended from a launchd agent every 15 minutes, and to be safe to run by
# hand any time.
#
# Why this exists: agent/automation flows (rules-review, verify, composer-mode,
# MCP builds) point CARGO_TARGET_DIR / Xcode DerivedData into /private/tmp and
# never clean up. Each run drops a 5-14G build dir there; they pile to 100G+ and
# fill the disk invisibly. Project target/ dirs and ~/Library/Caches add more.
#
# Tiers, deleted in order, stopping as soon as free >= TARGET_GB:
#   1. /private/tmp        - throwaway scratch + redirected build dirs (the big one)
#   2. ~/dev build caches  - Rust target*/.build/DerivedData (forces a recompile)
#   3. ~/Library/Caches    - app + tool caches (npm/gradle/cargo/generic)
#
# Nothing here is irreplaceable: every target is a build output or cache that
# regenerates on next build. Two guards keep it from breaking live work:
#   - it only acts when free space is below THRESHOLD_GB, and
#   - it never deletes anything modified in the last IDLE_MIN minutes (so an
#     in-flight build's target dir is left alone).
#
# Usage:
#   reclaim.sh                 # act only if free < THRESHOLD_GB
#   reclaim.sh --force         # run every tier regardless of free space
#   reclaim.sh --dry-run       # show what would be deleted, delete nothing
#   reclaim.sh --threshold 50  # override low-water mark (GB)
#   reclaim.sh --target 80     # override stop-at free space (GB)
#   reclaim.sh --verbose       # log even when no action is taken

set -uo pipefail

THRESHOLD_GB="${RECLAIM_THRESHOLD_GB:-40}"   # act when free drops below this
TARGET_GB="${RECLAIM_TARGET_GB:-60}"         # delete until free reaches this
IDLE_MIN="${RECLAIM_IDLE_MIN:-30}"           # never touch anything newer than this
LOG="${RECLAIM_LOG:-$HOME/Library/Logs/reclaim-disk.log}"

DRY_RUN=0
FORCE=0
VERBOSE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --force) FORCE=1 ;;
    --verbose) VERBOSE=1 ;;
    --threshold) THRESHOLD_GB="$2"; shift ;;
    --target) TARGET_GB="$2"; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done

mkdir -p "$(dirname "$LOG")"

log() { printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" >>"$LOG"; }

# Available GB on the volume backing / (the one that fills).
avail_gb() { df -g / | awk 'NR==2 {print $4}'; }

# Size of a path in human form, for logging. Empty if it doesn't exist.
size_of() { du -sh "$1" 2>/dev/null | cut -f1; }

# Remove a single path (file or dir), logging its size. Honors --dry-run.
purge() {
  local p="$1" sz
  [[ -e "$p" ]] || return 0
  sz="$(size_of "$p")"
  if [[ "$DRY_RUN" == 1 ]]; then
    log "DRY would remove ${sz:-?}  $p"
  else
    rm -rf "$p" && log "removed ${sz:-?}  $p"
  fi
}

reached_target() { [[ "$(avail_gb)" -ge "$TARGET_GB" ]]; }

# Tier 1: /private/tmp top-level entries older than IDLE_MIN, except the live
# Claude Code session scratch (claude-*) and macOS's own runtime dirs
# (com.apple.*): launchd keeps per-session socket dirs there
# (com.apple.launchd.* — testmanagerd, auth services), and deleting one
# breaks simulator UI testing and anything else on those sockets until the
# session restarts. They hold sockets, not disk weight. This is where the
# redirected build dirs accumulate, so it's both the biggest win and the
# safest.
tier_tmp() {
  log "tier 1: /private/tmp (idle > ${IDLE_MIN}m)"
  local p
  while IFS= read -r -d '' p; do
    purge "$p"
    reached_target && return 0
  done < <(find /private/tmp -maxdepth 1 -mindepth 1 \
             ! -name 'claude-*' ! -name 'com.apple.*' \
             -mmin "+${IDLE_MIN}" -print0 2>/dev/null)
}

# Tier 2: Rust/Swift/Xcode build output dirs under ~/dev, idle > IDLE_MIN.
# Deleting these only costs a recompile. node_modules is intentionally excluded
# (it needs a reinstall, not a rebuild, and isn't "build output").
tier_builds() {
  log "tier 2: ~/dev build caches (idle > ${IDLE_MIN}m)"
  local p
  while IFS= read -r -d '' p; do
    purge "$p"
    reached_target && return 0
  done < <(find "$HOME/dev" -maxdepth 5 -type d \
             \( -name target -o -name 'target-*' -o -name .build -o -name DerivedData \) \
             -mmin "+${IDLE_MIN}" -prune -print0 2>/dev/null)
}

# A tool's cache must not be swept out from under a live build: deleting
# ~/.gradle/caches while a Gradle daemon runs corrupts its kotlin-dsl script
# cache mid-build, and cargo's registry mid-compile fails the build. Skip a
# cache whose owning tool is currently running.
tool_cache_in_use() {
  case "$1" in
    "$HOME/.gradle/caches") pgrep -qf GradleDaemon ;;
    "$HOME/.cargo/registry/"*) pgrep -qx cargo ;;
    *) return 1 ;;
  esac
}

# Tier 3: app + tool caches. Contents only; the parent dirs are recreated by
# their owners on next use.
tier_caches() {
  log "tier 3: ~/Library/Caches and tool caches"
  local d
  for d in "$HOME/Library/Caches"/* "$HOME/.cache"/* \
           "$HOME/.npm/_cacache" "$HOME/.gradle/caches" \
           "$HOME/.cargo/registry/cache" "$HOME/.cargo/registry/src"; do
    [[ -e "$d" ]] || continue
    if tool_cache_in_use "$d"; then
      log "skip (owner running): $d"
      continue
    fi
    purge "$d"
    reached_target && return 0
  done
}

main() {
  local start free_start
  start="$(avail_gb)"
  free_start="$start"

  if [[ "$FORCE" != 1 && "$start" -ge "$THRESHOLD_GB" ]]; then
    [[ "$VERBOSE" == 1 ]] && log "ok: ${start}G free >= ${THRESHOLD_GB}G threshold, nothing to do"
    return 0
  fi

  log "START free=${start}G threshold=${THRESHOLD_GB}G target=${TARGET_GB}G force=${FORCE} dry=${DRY_RUN}"

  tier_tmp;    reached_target && { log "DONE free=$(avail_gb)G (reclaimed $(( $(avail_gb) - free_start ))G)"; return 0; }
  tier_builds; reached_target && { log "DONE free=$(avail_gb)G (reclaimed $(( $(avail_gb) - free_start ))G)"; return 0; }
  tier_caches

  local end; end="$(avail_gb)"
  log "DONE free=${end}G (reclaimed $(( end - free_start ))G)"
  if [[ "$end" -lt "$THRESHOLD_GB" ]]; then
    log "WARN still below ${THRESHOLD_GB}G after all tiers - remaining usage is real data, not build junk"
  fi
}

main

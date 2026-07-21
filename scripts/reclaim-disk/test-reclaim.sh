#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
subject="$script_dir/reclaim.sh"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

home="$scratch/home"
log="$scratch/reclaim.log"
mkdir -p "$home/dev/recent-build/target/debug" "$home/Library/Caches"
touch "$home/dev/recent-build/target/debug/fingerprint"
touch -t 202001010000 "$home/dev/recent-build/target"

HOME="$home" \
RECLAIM_LOG="$log" \
RECLAIM_IDLE_MIN=30 \
RECLAIM_THRESHOLD_GB=9999 \
RECLAIM_TARGET_GB=9999 \
  "$subject" --force --dry-run

if grep -F "DRY would remove" "$log" | grep -Fq "$home/dev/recent-build/target"; then
  echo "recent descendant did not protect its build output" >&2
  exit 1
fi

mkdir -p "$home/dev/live-build/target"
touch -t 202001010000 "$home/dev/live-build/target"
ln -s /bin/sleep "$scratch/cargo"
(
  cd "$home/dev/live-build"
  "$scratch/cargo" 30
) &
build_pid=$!
trap 'kill "$build_pid" 2>/dev/null || true; rm -rf "$scratch"' EXIT

: >"$log"
HOME="$home" \
RECLAIM_LOG="$log" \
RECLAIM_IDLE_MIN=30 \
RECLAIM_THRESHOLD_GB=9999 \
RECLAIM_TARGET_GB=9999 \
  "$subject" --force --dry-run

if grep -F "DRY would remove" "$log" | grep -Fq "$home/dev/live-build/target"; then
  echo "running build did not protect its build output" >&2
  exit 1
fi

kill "$build_pid" 2>/dev/null || true
wait "$build_pid" 2>/dev/null || true

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
trap 'rm -rf "$scratch"' EXIT

# Redirected CARGO_TARGET_DIR roots: the per-project dirs under them are build
# output too, and the checkout that owns one is nowhere on its path. A cold one
# goes; a live one is held by a build with files open under it. Both dirs and
# the lock file are backdated so has_recent_descendant can't be what protects
# the live one — this has to exercise the open-files check specifically, and the
# build runs from a cwd outside the dir so cwd can't be what matches either.
mkdir -p "$home/.cargo-target/coldproj/debug" "$home/.cargo-target/liveproj/debug"
touch "$home/.cargo-target/liveproj/debug/.cargo-lock"
touch -t 202001010000 "$home/.cargo-target/liveproj/debug/.cargo-lock"
touch -t 202001010000 \
  "$home/.cargo-target/coldproj/debug" "$home/.cargo-target/coldproj" \
  "$home/.cargo-target/liveproj/debug" "$home/.cargo-target/liveproj" \
  "$home/.cargo-target"

(
  cd "$scratch"
  exec 9<"$home/.cargo-target/liveproj/debug/.cargo-lock"
  exec "$scratch/cargo" 30
) &
redirected_pid=$!
trap 'kill "$redirected_pid" 2>/dev/null || true; rm -rf "$scratch"' EXIT
sleep 1

: >"$log"
HOME="$home" \
RECLAIM_LOG="$log" \
RECLAIM_IDLE_MIN=30 \
RECLAIM_THRESHOLD_GB=9999 \
RECLAIM_TARGET_GB=9999 \
  "$subject" --force --dry-run

if ! grep -F "DRY would remove" "$log" | grep -Fq "$home/.cargo-target/coldproj"; then
  echo "idle redirected target dir was not swept" >&2
  exit 1
fi

if grep -F "DRY would remove" "$log" | grep -Fq "$home/.cargo-target/liveproj"; then
  echo "build holding files open did not protect its redirected target dir" >&2
  exit 1
fi

kill "$redirected_pid" 2>/dev/null || true
wait "$redirected_pid" 2>/dev/null || true

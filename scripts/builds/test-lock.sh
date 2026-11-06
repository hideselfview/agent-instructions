#!/usr/bin/env bash
# Tests `builds lock` on a scratch build volume (BUILDS_ROOT) in a temporary
# folder, with `sleep` standing in for a build: a waiting command does not
# start while any process of the holder's build lives, whether the holder is
# killed outright or asked to stop.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cargo build --quiet --manifest-path "$script_dir/Cargo.toml"
builds="$script_dir/target/debug/builds"
scratch="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$scratch"' EXIT
export BUILDS_ROOT="$scratch/volume"
mkdir -p "$BUILDS_ROOT/demo/main"
echo side-1 > "$BUILDS_ROOT/demo/app-checkout"
# A build root, so health lists the repo and reads its app lock.
echo "$scratch/gone" > "$BUILDS_ROOT/demo/main/.checkout"

fail() { echo "FAIL: $*" >&2; exit 1; }

# A repo without an app checkout has no app lock to take.
if "$builds" lock other true 2>/dev/null; then fail "lock took a lock for a repo with no app checkout"; fi

# The waiter: says whether the build had finished (<file> exists) when it
# started.
waiter() {
    "$builds" lock demo sh -c 'if [ -f "$1" ]; then echo after; else echo during; fi > "$2"' sh "$1" "$2" &
}

# 1. The holder killed outright (SIGKILL: nothing is passed on). Its build
# keeps running and keeps the lock until it ends.
mkfifo "$scratch/started"
"$builds" lock demo sh -c 'echo > "$1/started"; sleep 3; touch "$1/build.done"' sh "$scratch" &
holder=$!
read -r _ < "$scratch/started"
waiter "$scratch/build.done" "$scratch/waiter1"
waiting=$!
kill -KILL "$holder"
{ wait "$holder"; } 2>/dev/null || true
wait "$waiting"
[ "$(cat "$scratch/waiter1")" = after ] || fail "the waiter started while the killed holder's build still ran"

# 2. The holder asked to stop (SIGTERM). It passes the signal on to its
# build's whole process group, the background sleep included, and the waiter
# starts once all of it has exited, well before the sleep would have ended.
"$builds" lock demo sh -c 'trap "touch \"$1/build.stopped\"; exit 1" TERM; sleep 60 & echo > "$1/started"; wait' sh "$scratch" &
holder=$!
read -r _ < "$scratch/started"
waiter "$scratch/build.stopped" "$scratch/waiter2"
waiting=$!
# health names the `builds lock` with the file open as the holder.
held_by="$("$builds" health --json | python3 -c 'import json, sys; print(json.load(sys.stdin)["locks"][0]["holder"]["pid"])' || true)"
[ "$held_by" = "$holder" ] || fail "health named $held_by as the lock's holder, not $holder"
SECONDS=0
kill -TERM "$holder"
status=0
wait "$holder" || status=$?
wait "$waiting"
[ "$(cat "$scratch/waiter2")" = after ] || fail "the waiter started before the stopped build had exited"
[ "$status" != 0 ] || fail "the holder exited 0 for a build that was stopped"
[ "$SECONDS" -lt 30 ] || fail "the build's background sleep was not stopped ($SECONDS s)"

echo ok

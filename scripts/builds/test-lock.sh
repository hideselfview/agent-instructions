#!/usr/bin/env bash
# Tests `builds lock` on a scratch repo and build volume (BUILDS_ROOT) in a
# temporary folder, with `sleep` standing in for a build: a waiting command
# does not start while any process of the holder's build lives, whether the
# holder is killed outright or asked to stop; and health counts a build under
# the lock as the build of the worktree it is for, and finds the holder by the
# lock file's open descriptor once `builds lock` itself is gone.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cargo build --quiet --manifest-path "$script_dir/Cargo.toml"
builds="$script_dir/target/debug/builds"
scratch="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$scratch"' EXIT
export BUILDS_ROOT="$scratch/volume"

# A repo demo with origin/main, its app checkout side-1, and side-2, claimed,
# whose one commit of its own was made three hours ago and nothing since.
main="$scratch/demo"
app="$scratch/demo-worktrees/side-1"
side="$scratch/demo-worktrees/side-2"
old="$(date -v-3H +%s) +0000"
git init -q -b main "$main"
git -C "$main" config user.email test@example.com
git -C "$main" config user.name test
printf 'target 1G\n' > "$main/.build-dirs"
echo target > "$main/.gitignore"
git -C "$main" add .build-dirs .gitignore
GIT_AUTHOR_DATE="$old" GIT_COMMITTER_DATE="$old" git -C "$main" commit -q -m Start
git init -q --bare "$scratch/origin.git"
git -C "$main" remote add origin "$scratch/origin.git"
git -C "$main" push -q origin main
git -C "$main" worktree add -q --detach "$app"
git -C "$main" worktree add -q -b work "$side" origin/main
echo change > "$side/file"
git -C "$side" add file
GIT_AUTHOR_DATE="$old" GIT_COMMITTER_DATE="$old" git -C "$side" commit -q -m Work
work="$(git -C "$side" rev-parse HEAD)"
for top in "$main" "$app" "$side"; do
    if [ "$top" = "$main" ]; then name=main; else name="$(basename "$top")"; fi
    mkdir -p "$BUILDS_ROOT/demo/$name"
    echo "$top" > "$BUILDS_ROOT/demo/$name/.checkout"
done
"$builds" app-checkout mac "$app" > /dev/null 2>&1
"$builds" claim "$side" "agent B" > /dev/null 2>&1

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

# health's view, as JSON; it exits 1 when it finds something to act on.
health() { "$builds" health --json || true; }
# The JSON value at <python expression on d>.
json() { python3 -c 'import json, sys; d = json.load(sys.stdin); print(eval(sys.argv[1]))' "$1"; }
side_2='[w for w in d["worktrees"] if w["checkout"] == "demo/side-2"][0]'
idle='any(f.startswith("worktree: demo/side-2 ") for f in d["findings"])'

# 3. side-2, quiet for three hours with no build of its own, is idle.
[ "$(health | json "$idle")" = True ] || fail "health did not call the quiet side-2 idle"

# 4. A `builds lock` run from side-2 builds for it: side-2 is not idle.
mkfifo "$scratch/started3"
(cd "$side" && exec "$builds" lock demo sh -c 'echo > "$1"; exec sleep 30' sh "$scratch/started3") &
holder=$!
read -r _ < "$scratch/started3"
seen="$(health)"
[ "$(json "$side_2[\"app_build\"]" <<< "$seen")" = "$holder" ] || fail "health did not link the lock run from side-2 to it"
[ "$(json "$idle" <<< "$seen")" = False ] || fail "health called side-2 idle while its build held the app lock"
kill -TERM "$holder"
{ wait "$holder"; } 2>/dev/null || true

# 5. A `builds lock` run from elsewhere that moves the app checkout to
# side-2's commit, then killed outright: its build still holds the lock, and
# health finds that build by the open lock file and links it to side-2
# through the commit.
mkfifo "$scratch/started4"
(cd "$scratch" && exec "$builds" lock demo sh -c 'cd "$1" && git checkout -q --detach "$2" && echo > "$3" && sleep 30' sh "$app" "$work" "$scratch/started4") &
holder=$!
read -r _ < "$scratch/started4"
build="$(pgrep -P "$holder")"
kill -KILL "$holder"
{ wait "$holder"; } 2>/dev/null || true
seen="$(health)"
[ "$(json 'd["locks"][0]["holder"]["pid"]' <<< "$seen")" = "$build" ] || fail "health did not find the killed holder's build ($build) by the open lock file"
[ "$(json "$side_2[\"app_build\"]" <<< "$seen")" = "$build" ] || fail "health did not link the app checkout's build at side-2's commit to side-2"
[ "$(json "$idle" <<< "$seen")" = False ] || fail "health called side-2 idle while a build at its commit held the app lock"
kill -TERM -- "-$build"

echo ok

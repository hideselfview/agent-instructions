#!/usr/bin/env bash
# Tests claims, their notes and the notes' states, `status --json` and the
# dashboard's answers, the states it derives among them, on a scratch repo
# with two side worktrees and a scratch build volume (BUILDS_ROOT) in a
# temporary folder: nothing here touches the real build volume or any real
# checkout.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cargo build --quiet --manifest-path "$script_dir/Cargo.toml"
builds="$script_dir/target/debug/builds"
scratch="$(cd "$(mktemp -d)" && pwd -P)"
server=""
watcher=""
cleanup() {
    [ -z "$server" ] || kill "$server" 2>/dev/null || true
    [ -z "$watcher" ] || kill "$watcher" 2>/dev/null || true
    rm -rf "$scratch"
}
trap cleanup EXIT
export BUILDS_ROOT="$scratch/volume"

fail() { echo "FAIL: $*" >&2; exit 1; }
# Runs builds, which must fail, and checks it said <text>.
refuses() {
    local text="$1" out
    shift
    if out="$("$builds" "$@" 2>&1)"; then fail "builds $* should have failed"; fi
    case "$out" in *"$text"*) ;; *) fail "builds $* said '$out', not '$text'" ;; esac
}
# The value at <python expression on d> in the JSON on stdin.
json() { python3 -c 'import json, sys; d = json.load(sys.stdin); print(json.dumps(eval(sys.argv[1])))' "$1"; }

# A repo whose origin/main has one commit; side-1 on a branch two commits
# ahead of it, with one changed and one untracked file; side-2 on a branch
# whose one commit landed on origin/main by cherry-pick, under another hash;
# side-3 on a branch whose one commit landed the same way but with a changed
# neighbouring line, so its patch differs and only its author time and subject
# match.
# Each checkout has the build root and link `builds setup` would make (setup
# itself also prunes the real temporary folders, so it isn't run here).
git init -q --bare "$scratch/origin.git"
main="$scratch/demo"
side="$scratch/demo-worktrees/side-1"
side2="$scratch/demo-worktrees/side-2"
side3="$scratch/demo-worktrees/side-3"
git init -q -b main "$main"
git -C "$main" config user.email test@example.com
git -C "$main" config user.name test
echo 'target 1G' > "$main/.build-dirs"
echo target > "$main/.gitignore"
seq 1 10 > "$main/ctx"
git -C "$main" add .build-dirs .gitignore ctx
git -C "$main" commit -q -m 'Start'
git -C "$main" remote add origin "$scratch/origin.git"
git -C "$main" push -q origin main
git -C "$main" fetch -q origin
mkdir -p "$scratch/demo-worktrees"
git -C "$main" worktree add -q -b work "$side" origin/main
git -C "$main" worktree add -q -b landed "$side2" origin/main
git -C "$main" worktree add -q -b landed-moved "$side3" origin/main
for top in "$main" "$side" "$side2" "$side3"; do
    if [ "$top" = "$main" ]; then root="$BUILDS_ROOT/demo/main"; else root="$BUILDS_ROOT/demo/$(basename "$top")"; fi
    mkdir -p "$root/target"
    printf '%s\n' "$top" > "$root/.checkout"
    ln -s "$root/target" "$top/target"
done
head -c 2097152 /dev/urandom > "$BUILDS_ROOT/demo/side-1/target/blob"
echo one > "$side/one" && git -C "$side" add one && git -C "$side" commit -q -m 'Add one'
echo two > "$side/two" && git -C "$side" add two && git -C "$side" commit -q -m 'Add two'
echo changed > "$side/one"
echo new > "$side/untracked"
echo three > "$side2/three" && git -C "$side2" add three && git -C "$side2" commit -q -m 'Add three'
sed -i.bak 5s/5/five/ "$side3/ctx" && rm "$side3/ctx.bak" && git -C "$side3" commit -q -am 'Rename five'
echo other > "$main/other" && git -C "$main" add other && git -C "$main" commit -q -m 'Move main on'
sed -i.bak 7s/7/seven/ "$main/ctx" && rm "$main/ctx.bak" && git -C "$main" commit -q -am 'Rename seven'
git -C "$main" cherry-pick landed landed-moved >/dev/null
git -C "$main" push -q origin main
git -C "$main" fetch -q origin
[ -n "$(git -C "$side2" log --format=%h origin/main..HEAD)" ] \
    || fail "side-2's landed commit should still be off origin/main by hash"
[ "$(git -C "$side3" cherry origin/main HEAD | cut -c1)" = + ] \
    || fail "side-3's landed commit should differ in patch from its copy"
claim_file="$(git -C "$side" rev-parse --absolute-git-dir)/builds-claim"

# A note needs a claim, and main is never claimed.
refuses "has no claim" note "$side" waiting
refuses "main checkout" claim "$main" someone
refuses "has no claim" note "$main" waiting

"$builds" claim "$side" "agent A" 2>/dev/null
[ "$(cat "$claim_file")" = "agent A	$(cut -f2 "$claim_file")" ] || fail "claim wrote '$(cat "$claim_file")'"
since="$(cut -f2 "$claim_file")"

"$builds" note "$side" waiting on Linux CI 2>/dev/null
[ "$(cat "$claim_file")" = "agent A	$since	waiting on Linux CI" ] || fail "note wrote '$(cat "$claim_file")'"
status="$("$builds" status)"
grep -q '^demo/side-1 .*\[work, claimed by agent A since '"$since"'\]$' <<<"$status" \
    || fail "status doesn't show the claim: $status"
grep -A1 '^demo/side-1 ' <<<"$status" | grep -q '^    note: waiting on Linux CI$' \
    || fail "status doesn't show the note under its checkout: $status"

# Claiming again for the same who renews it and keeps the note; another who
# is refused.
"$builds" claim "$side" "agent A" 2>/dev/null
[ "$(cut -f1,3 "$claim_file")" = "agent A	waiting on Linux CI" ] || fail "re-claim wrote '$(cat "$claim_file")'"
refuses "claimed by agent A" claim "$side" "agent B"
refuses "one line without tabs" note "$side" "$(printf 'a\tb')"
refuses "one line without tabs" claim "$side" "$(printf 'agent\tA')"

# A note's state: a fourth field, shown by status, kept by a re-claim and
# cleared by a note without one. (The re-claim above renewed the date.)
since="$(cut -f2 "$claim_file")"
"$builds" note "$side" --state waiting-on-you which schema to keep 2>/dev/null
[ "$(cat "$claim_file")" = "agent A	$since	which schema to keep	waiting-on-you" ] \
    || fail "note --state wrote '$(cat "$claim_file")'"
grep -A1 '^demo/side-1 ' <<<"$("$builds" status)" | grep -q '^    note (waiting on you): which schema to keep$' \
    || fail "status doesn't show the note's state: $("$builds" status)"
[ "$("$builds" status --json --no-folders | json 'd["repos"][0]["checkouts"][1]["claim"]')" \
    = '{"who": "agent A", "since": "'"$since"'", "note": "which schema to keep", "note_state": "waiting-on-you"}' ] \
    || fail "note state in json: $("$builds" status --json --no-folders)"
"$builds" claim "$side" "agent A" 2>/dev/null
[ "$(cut -f1,3,4 "$claim_file")" = "agent A	which schema to keep	waiting-on-you" ] || fail "re-claim dropped the state: '$(cat "$claim_file")'"
since="$(cut -f2 "$claim_file")"
refuses "needs the note's text" note "$side" --state winding-down
refuses "is one of waiting-on-you, winding-down, held, got 'done'" note "$side" --state done finished
refuses "is one of waiting-on-you, winding-down, held, got ''" note "$side" --state
[ "$(cut -f4 "$claim_file")" = waiting-on-you ] || fail "a refused note changed the claim: '$(cat "$claim_file")'"
"$builds" note "$side" waiting on Linux CI 2>/dev/null
[ "$(cat "$claim_file")" = "agent A	$since	waiting on Linux CI" ] || fail "a plain note kept the state: '$(cat "$claim_file")'"
[ "$("$builds" status --json --no-folders | json 'd["repos"][0]["checkouts"][1]["claim"]["note_state"]')" = null ] \
    || fail "a plain note's state in json"

# status --json, with and without the folders.
"$builds" status --json --no-folders > "$scratch/fast.json"
[ "$(json 'd["repos"][0]["name"]' < "$scratch/fast.json")" = '"demo"' ] || fail "no demo repo in $(cat "$scratch/fast.json")"
[ "$(json '[c["name"] for c in d["repos"][0]["checkouts"]]' < "$scratch/fast.json")" = '["main", "side-1", "side-2", "side-3"]' ] \
    || fail "checkouts: $(cat "$scratch/fast.json")"
[ "$(json 'd["repos"][0]["checkouts"][1]["claim"]["note"]' < "$scratch/fast.json")" = '"waiting on Linux CI"' ] \
    || fail "note in json: $(cat "$scratch/fast.json")"
[ "$(json 'd["repos"][0]["checkouts"][1]["folders"]' < "$scratch/fast.json")" = null ] || fail "--no-folders listed folders"
[ "$(json 'd["repos"][0]["checkouts"][0]["branch"]' < "$scratch/fast.json")" = '"main"' ] || fail "main's branch"
"$builds" status --json > "$scratch/full.json"
[ "$(json 'd["repos"][0]["checkouts"][1]["folders"][0]["link"]' < "$scratch/full.json")" = '"linked"' ] \
    || fail "folder in json: $(cat "$scratch/full.json")"
[ "$(json 'd["repos"][0]["checkouts"][1]["total_kib"] >= 2048' < "$scratch/full.json")" = true ] \
    || fail "side-1's size: $(cat "$scratch/full.json")"
refuses "usage: builds status" status --sizes

# A build folder's last write is its newest write inside.
touch -t 202001010000 "$BUILDS_ROOT/demo/side-1/target"
touch -t 202101010000 "$BUILDS_ROOT/demo/side-1/target/blob"
[ "$("$builds" status --json | json 'd["repos"][0]["checkouts"][1]["folders"][0]["last_write"]')" \
    = "$(date -j -f %Y%m%d%H%M%S 20210101000000 +%s)" ] || fail "side-1's target's last write: $("$builds" status --json)"
touch "$BUILDS_ROOT/demo/side-1/target/blob"

# The daemon follows the scratch volume; status asks it for the
# sizes, and says the same as when it measured them itself. A second daemon
# on the same volume is refused. It serves the page, the state the page shows,
# and the same state as events when anything changes; it refuses a Host it
# doesn't serve. It runs until the states below are checked.
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
"$builds" dashboard --port "$port" 2>"$scratch/daemon.log" &
server=$!
for _ in $(seq 100); do
    grep -q 'following ' "$scratch/daemon.log" && break
    sleep 0.1
done
grep -q 'following ' "$scratch/daemon.log" || fail "the daemon did not follow the volume: $(cat "$scratch/daemon.log")"
"$builds" status --json > "$scratch/daemon.json"
[ "$(json '[(c["name"], c["total_kib"]) for c in d["repos"][0]["checkouts"]]' < "$scratch/daemon.json")" \
    = "$(json '[(c["name"], c["total_kib"]) for c in d["repos"][0]["checkouts"]]' < "$scratch/full.json")" ] \
    || fail "the daemon's sizes differ: $(cat "$scratch/daemon.json") against $(cat "$scratch/full.json")"
head -c 1048576 /dev/urandom > "$BUILDS_ROOT/demo/side-1/target/more"
"$builds" prune --dry-run 2>/dev/null
[ "$(json 'd["repos"][0]["checkouts"][1]["total_kib"] >= 3072' < <("$builds" status --json))" = true ] \
    || fail "the daemon missed a write: $("$builds" status --json)"
rm "$BUILDS_ROOT/demo/side-1/target/more"
port2="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
refuses "a daemon already answers on" dashboard --port "$port2"

curl -sf "http://127.0.0.1:$port/" > "$scratch/page.html" || fail "no page: $(cat "$scratch/daemon.log")"
grep -q '<title>Builds</title>' "$scratch/page.html" || fail "the page has no title Builds"
# The first state may come before the checkouts are read.
for _ in $(seq 100); do
    curl -sf "http://127.0.0.1:$port/api/state" > "$scratch/state.json" || fail "no state: $(cat "$scratch/daemon.log")"
    [ "$(json 'len(d["repos"]) == 1 and d["repos"][0]["checkouts"][1]["git"] is not None and d["repos"][0]["checkouts"][1]["total_kib"] is not None' < "$scratch/state.json")" = true ] && break
    sleep 0.1
done
[ "$(json '[x["subject"] for x in d["repos"][0]["checkouts"][1]["git"]["ahead"]]' < "$scratch/state.json")" = '["Add two", "Add one"]' ] \
    || fail "commits waiting to land: $(cat "$scratch/state.json")"
[ "$(json 'd["repos"][0]["checkouts"][1]["git"]["uncommitted"]' < "$scratch/state.json")" = 2 ] || fail "uncommitted count"
[ "$(json 'abs(d["repos"][0]["checkouts"][1]["git"]["last_write"] - d["generated_at"]) < 600' < "$scratch/state.json")" = true ] \
    || fail "uncommitted files' newest write: $(cat "$scratch/state.json")"
[ "$(json 'd["repos"][0]["checkouts"][2]["git"]["last_write"]' < "$scratch/state.json")" = null ] || fail "a clean worktree has a last write"
[ "$(json 'd["repos"][0]["checkouts"][1]["git"]["head"]["subject"]' < "$scratch/state.json")" = '"Add two"' ] || fail "HEAD"
[ "$(json 'd["repos"][0]["checkouts"][0]["git"]' < "$scratch/state.json")" = null ] || fail "asked git about main"
[ "$(json 'd["repos"][0]["checkouts"][1]["state"]' < "$scratch/state.json")" = '"ready-to-land"' ] || fail "side-1's state"
[ "$(json 'd["repos"][0]["checkouts"][0]["state"]' < "$scratch/state.json")" = null ] || fail "main got a state"
[ "$(json 'd["repos"][0]["checkouts"][2]["git"]["ahead"]' < "$scratch/state.json")" = '[]' ] \
    || fail "side-2's cherry-picked commit listed as waiting: $(cat "$scratch/state.json")"
[ "$(json 'd["repos"][0]["checkouts"][3]["git"]["ahead"]' < "$scratch/state.json")" = '[]' ] \
    || fail "side-3's landed commit with a changed patch listed as waiting: $(cat "$scratch/state.json")"
[ "$(json 'd["repos"][0]["checkouts"][1]["total_kib"] >= 2048' < "$scratch/state.json")" = true ] \
    || fail "side-1's size in the state: $(cat "$scratch/state.json")"
[ "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: example.com' "http://127.0.0.1:$port/api/state")" = 403 ] \
    || fail "served a foreign Host"

# A note written while the page watches reaches it as an event, along
# with the load the daemon reads while a page watches.
curl -sN "http://127.0.0.1:$port/events" > "$scratch/events.txt" &
watcher=$!
for _ in $(seq 100); do
    grep -q '^event: state' "$scratch/events.txt" && break
    sleep 0.1
done
grep -q '^event: state' "$scratch/events.txt" || fail "no state event"
"$builds" note "$side" seen by the page 2>/dev/null
for _ in $(seq 100); do
    grep -q '"note":"seen by the page"' "$scratch/events.txt" && break
    sleep 0.1
done
grep -q '"note":"seen by the page"' "$scratch/events.txt" || fail "the note never reached the page"
grep -q '"load":{"now":\[' "$scratch/events.txt" || fail "no load while the page watched"
kill "$watcher"
wait "$watcher" 2>/dev/null || true
watcher=""
"$builds" note "$side" waiting on Linux CI 2>/dev/null

# Sizes count each block once: a folder holding an APFS clone (cp -c) counts
# the shared blocks once, where du counts them twice; one call measures every
# path, in order, each on its own.
mkdir "$scratch/usage" "$scratch/usage/clones" "$scratch/usage/plain"
head -c 4194304 /dev/urandom > "$scratch/usage/clones/a"
cp -c "$scratch/usage/clones/a" "$scratch/usage/clones/b"
head -c 4194304 /dev/urandom > "$scratch/usage/plain/a"
sizes=($("$builds" du "$scratch/usage/clones" "$scratch/usage/missing" "$scratch/usage/plain"))
[ "${#sizes[@]}" = 3 ] || fail "measuring printed ${sizes[*]}"
[ "${sizes[0]}" -ge 4096 ] && [ "${sizes[0]}" -lt 4500 ] || fail "a folder with a clone took ${sizes[0]} KiB, not the 4096 of its shared blocks"
[ "${sizes[1]}" = 0 ] || fail "a missing path took ${sizes[1]} KiB"
[ "${sizes[2]}" -ge 4096 ] && [ "${sizes[2]}" -lt 4500 ] || fail "a plain 4 MiB file took ${sizes[2]} KiB"
[ "$(du -sk "$scratch/usage/clones" | cut -f1)" -ge 8192 ] || echo "note: du didn't double-count the clone here" >&2


# Clearing the note keeps the claim.
"$builds" note "$side" 2>/dev/null
[ "$(cat "$claim_file")" = "agent A	$since" ] || fail "clearing wrote '$(cat "$claim_file")'"

# The state the dashboard derives for each side worktree, from its claim, the
# note's state and the commits waiting to land: <expected side-1> <expected
# side-2>.
states() {
    local got
    # The daemon reads the change from its events, a moment after the
    # command wrote it.
    for _ in $(seq 100); do
        got="$(curl -sf "http://127.0.0.1:$port/api/state" | python3 -c 'import json, sys
print(" ".join(str(c["state"]) for c in json.load(sys.stdin)["repos"][0]["checkouts"]))')"
        [ "$got" = "None $1 $2 free" ] && return 0
        sleep 0.1
    done
    [ "$got" = "None $1 $2 free" ] || fail "states: wanted main None, side-1 $1, side-2 $2, side-3 free, got '$got'"
}
states ready-to-land free
"$builds" claim "$side2" "agent B" 2>/dev/null
states ready-to-land working
"$builds" note "$side2" --state winding-down last item, then the handoff 2>/dev/null
"$builds" note "$side" --state winding-down last item 2>/dev/null
states ready-to-land winding-down
"$builds" note "$side" --state waiting-on-you which schema to keep 2>/dev/null
"$builds" note "$side2" --state waiting-on-you run the reset 2>/dev/null
states waiting-on-you waiting-on-you
"$builds" release "$side2" 2>/dev/null
"$builds" claim "$side2" "coordinator (landing)" 2>/dev/null
states waiting-on-you landing
# A held claim shows as held, commits waiting to land or not.
"$builds" note "$side" --state held kept until step 8 lands 2>/dev/null
grep -A1 '^demo/side-1 ' <<<"$("$builds" status)" | grep -q '^    note (held): kept until step 8 lands$' \
    || fail "status doesn't show the hold: $("$builds" status)"
# Health, asked right after, reads the claim as it is now.
[ "$("$builds" health --json | json '[w["claim"]["note_state"] for w in d["worktrees"] if w["checkout"] == "demo/side-1"]')" = '["held"]' ] \
    || fail "health missed the hold: $("$builds" health --json)"
states held landing
"$builds" release "$side" 2>/dev/null
[ ! -e "$claim_file" ] || fail "release left the claim"
states ready-to-land landing

# Once the daemon is gone, status measures again.
kill "$server"
wait "$server" 2>/dev/null || true
server=""
"$builds" status --json > "$scratch/after.json"
[ "$(json 'd["repos"][0]["checkouts"][1]["total_kib"] >= 2048' < "$scratch/after.json")" = true ] \
    || fail "status after the daemon stopped: $(cat "$scratch/after.json")"

# A claimed worktree's cargo folder over its cap is trimmed of incremental
# folders, not deleted whole or skipped; an unclaimed one is deleted whole; the
# app checkout's is not trimmed. (A dry run: a real prune also deletes build
# output in the machine's real temporary folders.)
echo 'target 1M' > "$side3/.build-dirs"
trim_profile="$BUILDS_ROOT/demo/side-3/target/debug"
mkdir -p "$trim_profile/.fingerprint"
for n in 1 2 3 4; do
    mkdir -p "$trim_profile/incremental/crate$n-id"
    head -c 524288 /dev/urandom > "$trim_profile/incremental/crate$n-id/data"
    touch -t "20260101000$n" "$trim_profile/incremental/crate$n-id/data"
done
before="$(find "$trim_profile" | sort)"
"$builds" claim "$side3" "agent C" 2>/dev/null
out="$("$builds" prune --dry-run 2>&1)"
grep -q "would delete 2 incremental folders of 4 in .*demo/side-3/target" <<<"$out" \
    || fail "a claimed folder over its cap wasn't trimmed: $out"
grep -q "claimed, would trim: .*demo/side-3/target (.* left, under the cap)" <<<"$out" \
    || fail "the trim said nothing of what is left: $out"
[ "$(find "$trim_profile" | sort)" = "$before" ] || fail "a dry run trimmed"
"$builds" app-checkout "$side3" 2>/dev/null
out="$("$builds" prune --dry-run 2>&1)"
# Its builds run without incremental compilation, so every incremental folder
# goes with the unused build copies, which leaves it under its cap.
! grep -q "would trim\|incremental folders of\|would delete .*demo/side-3/target (" <<<"$out" \
    || fail "the app checkout's claimed folder was trimmed: $out"
grep -q "would delete 4 incremental folders this checkout builds without incremental compilation from .*demo/side-3/target/debug" <<<"$out" \
    || fail "the app checkout's incremental folders were kept: $out"
rm "$BUILDS_ROOT/demo/app-checkout"
"$builds" release "$side3" 2>/dev/null
out="$("$builds" prune --dry-run 2>&1)"
grep -q "would delete .*demo/side-3/target (.* > 1M)" <<<"$out" || fail "an unclaimed folder over its cap wasn't deleted whole: $out"
git -C "$side3" checkout -q .build-dirs

echo "claims, notes and their states, status --json and the dashboard: ok"

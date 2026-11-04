#!/usr/bin/env bash
# Tests claims, their notes and the notes' states, `status --json` and the
# dashboard's answers, the states it derives among them, on a scratch repo
# with two side worktrees and a scratch build volume (BUILDS_ROOT) in a
# temporary folder: nothing here touches the real build volume or any real
# checkout.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
builds="$script_dir/builds"
scratch="$(cd "$(mktemp -d)" && pwd -P)"
server=""
cleanup() {
    [ -z "$server" ] || kill "$server" 2>/dev/null || true
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
# whose one commit landed on origin/main by cherry-pick, under another hash.
# Each checkout has the build root and link `builds setup` would make (setup
# itself also prunes the real temporary folders, so it isn't run here).
git init -q --bare "$scratch/origin.git"
main="$scratch/demo"
side="$scratch/demo-worktrees/side-1"
side2="$scratch/demo-worktrees/side-2"
git init -q -b main "$main"
git -C "$main" config user.email test@example.com
git -C "$main" config user.name test
echo 'target 1G' > "$main/.build-dirs"
echo target > "$main/.gitignore"
git -C "$main" add .build-dirs .gitignore
git -C "$main" commit -q -m 'Start'
git -C "$main" remote add origin "$scratch/origin.git"
git -C "$main" push -q origin main
git -C "$main" fetch -q origin
mkdir -p "$scratch/demo-worktrees"
git -C "$main" worktree add -q -b work "$side" origin/main
git -C "$main" worktree add -q -b landed "$side2" origin/main
for top in "$main" "$side" "$side2"; do
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
echo other > "$main/other" && git -C "$main" add other && git -C "$main" commit -q -m 'Move main on'
git -C "$main" cherry-pick landed >/dev/null
git -C "$main" push -q origin main
git -C "$main" fetch -q origin
[ -n "$(git -C "$side2" log --format=%h origin/main..HEAD)" ] \
    || fail "side-2's landed commit should still be off origin/main by hash"
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
refuses "is one of waiting-on-you, winding-down, got 'done'" note "$side" --state done finished
refuses "is one of waiting-on-you, winding-down, got ''" note "$side" --state
[ "$(cut -f4 "$claim_file")" = waiting-on-you ] || fail "a refused note changed the claim: '$(cat "$claim_file")'"
"$builds" note "$side" waiting on Linux CI 2>/dev/null
[ "$(cat "$claim_file")" = "agent A	$since	waiting on Linux CI" ] || fail "a plain note kept the state: '$(cat "$claim_file")'"
[ "$("$builds" status --json --no-folders | json 'd["repos"][0]["checkouts"][1]["claim"]["note_state"]')" = null ] \
    || fail "a plain note's state in json"

# status --json, with and without the folders.
"$builds" status --json --no-folders > "$scratch/fast.json"
[ "$(json 'd["repos"][0]["name"]' < "$scratch/fast.json")" = '"demo"' ] || fail "no demo repo in $(cat "$scratch/fast.json")"
[ "$(json '[c["name"] for c in d["repos"][0]["checkouts"]]' < "$scratch/fast.json")" = '["main", "side-1", "side-2"]' ] \
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

# disk_usage.py and newest_write.py: one call measures every path, in order,
# each on its own; a folder holding an APFS clone (cp -c) counts the shared
# blocks once, where du counts them twice.
mkdir "$scratch/usage" "$scratch/usage/clones" "$scratch/usage/plain"
head -c 4194304 /dev/urandom > "$scratch/usage/clones/a"
cp -c "$scratch/usage/clones/a" "$scratch/usage/clones/b"
head -c 4194304 /dev/urandom > "$scratch/usage/plain/a"
sizes=($(python3 "$script_dir/disk_usage.py" "$scratch/usage/clones" "$scratch/usage/missing" "$scratch/usage/plain"))
[ "${#sizes[@]}" = 3 ] || fail "disk_usage.py printed ${sizes[*]}"
[ "${sizes[0]}" -ge 4096 ] && [ "${sizes[0]}" -lt 4500 ] || fail "a folder with a clone took ${sizes[0]} KiB, not the 4096 of its shared blocks"
[ "${sizes[1]}" = 0 ] || fail "a missing path took ${sizes[1]} KiB"
[ "${sizes[2]}" -ge 4096 ] && [ "${sizes[2]}" -lt 4500 ] || fail "a plain 4 MiB file took ${sizes[2]} KiB"
[ "$(du -sk "$scratch/usage/clones" | cut -f1)" -ge 8192 ] || echo "note: du didn't double-count the clone here" >&2
touch -t 202001010000 "$scratch/usage/plain/a" "$scratch/usage/plain" "$scratch/usage/clones/a"
touch -t 202101010000 "$scratch/usage/clones/b"
writes=($(python3 "$script_dir/newest_write.py" "$scratch/usage/plain" "$scratch/usage/clones"))
[ "${writes[0]}" = "$(date -j -f %Y%m%d%H%M%S 20200101000000 +%s)" ] || fail "newest write of plain: ${writes[0]}"
[ "${writes[1]}" -gt "${writes[0]}" ] || fail "newest write of clones: ${writes[1]}"

# The dashboard: its answers for the side worktree, the page, and a Host it
# doesn't serve.
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
"$builds" dashboard --port "$port" 2>"$scratch/dashboard.log" &
server=$!
for _ in $(seq 50); do
    curl -sf "http://127.0.0.1:$port/" >/dev/null 2>&1 && break
    sleep 0.1
done
# Saved first: under pipefail, grep -q quitting at its match would fail curl
# still writing the page.
curl -sf "http://127.0.0.1:$port/" > "$scratch/page.html" || fail "no page: $(cat "$scratch/dashboard.log")"
grep -q '<title>Builds</title>' "$scratch/page.html" || fail "the page has no title Builds"
# /api/folders never waits on the measurement: the first request of the server's
# life, which is what asks for the measurement, gets the "measuring" marker.
curl -sf "http://127.0.0.1:$port/api/folders" > "$scratch/folders.json" || fail "no folders"
[ "$(cat "$scratch/folders.json")" = '{"measuring": true}' ] || fail "first folders answer: $(cat "$scratch/folders.json")"
curl -sf "http://127.0.0.1:$port/api/state" > "$scratch/state.json" || fail "no state"
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
# Later answers carry the data and the time it was taken.
for _ in $(seq 100); do
    curl -sf "http://127.0.0.1:$port/api/folders" > "$scratch/folders.json" || fail "no folders"
    [ "$(json '"measuring" in d' < "$scratch/folders.json")" = false ] && break
    sleep 0.2
done
[ "$(json 'd["repos"][0]["checkouts"][1]["total_kib"] >= 2048' < "$scratch/folders.json")" = true ] \
    || fail "folders' size: $(cat "$scratch/folders.json")"
[ "$(json 'd["generated_at"] > 0 and d["error"] is None' < "$scratch/folders.json")" = true ] || fail "folders' time"
[ "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: example.com' "http://127.0.0.1:$port/api/state")" = 403 ] \
    || fail "served a foreign Host"
kill "$server"
wait "$server" 2>/dev/null || true
server=""

# The measurement thread: with a stand-in measure, a run at once at start, one
# at a time, none once the asking stops, and a run as soon as a request finds
# the last result older than the interval.
python3 -B - "$script_dir" <<'PY' || fail "measurement thread"
import sys, time
sys.path.insert(0, sys.argv[1])
import dashboard

runs, active, overlap = [], [0], []
def measure():
    active[0] += 1
    if active[0] > 1:
        overlap.append(1)
    runs.append(time.monotonic())
    time.sleep(0.1)
    active[0] -= 1
    return {"repos": [], "generated_at": len(runs)}

m = dashboard.Measurement(measure, interval=0.5, window=0.3)
assert m.latest() == {"measuring": True}, "latest waited on the run"
for _ in range(20):
    m.want()
time.sleep(0.25)
assert len(runs) == 1 and m.latest()["generated_at"] == 1, f"no run at start: {runs}"
# Asked again inside the interval: the next run waits for the interval to end.
m.want()
time.sleep(0.1)
assert len(runs) == 1, "ran again before the interval ended"
# Asking stops (window 0.3s): nothing runs after the interval passes.
time.sleep(1.0)
assert len(runs) == 1, f"ran with nobody asking: {len(runs)}"
# A request now finds the result older than the interval: a run starts at once.
m.want()
time.sleep(0.05)
assert len(runs) == 2, f"no run on a request after the interval: {len(runs)}"
assert not overlap, "two runs at once"
PY

# Clearing the note keeps the claim.
"$builds" note "$side" 2>/dev/null
[ "$(cat "$claim_file")" = "agent A	$since" ] || fail "clearing wrote '$(cat "$claim_file")'"

# The state the dashboard derives for each side worktree, from its claim, the
# note's state and the commits waiting to land: <expected side-1> <expected
# side-2>.
states() {
    local got
    got="$(python3 -B -c 'import sys; sys.path.insert(0, sys.argv[1]); import dashboard
d = dashboard.state(sys.argv[2])
print(" ".join(str(c["state"]) for c in d["repos"][0]["checkouts"]))' "$script_dir" "$builds")"
    [ "$got" = "None $1 $2" ] || fail "states: wanted main None, side-1 $1, side-2 $2, got '$got'"
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
"$builds" release "$side" 2>/dev/null
[ ! -e "$claim_file" ] || fail "release left the claim"
states ready-to-land landing

echo "claims, notes and their states, status --json and the dashboard: ok"

#!/usr/bin/env bash
# Tests claims, their notes, `status --json` and the dashboard's answers on a
# scratch repo with a side worktree and a scratch build volume (BUILDS_ROOT)
# in a temporary folder: nothing here touches the real build volume or any
# real checkout.

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

# A repo whose origin/main has one commit, and a side worktree on a branch two
# commits ahead of it, with one changed and one untracked file. Each checkout
# has the build root and link `builds setup` would make (setup itself also
# prunes the real temporary folders, so it isn't run here).
git init -q --bare "$scratch/origin.git"
main="$scratch/demo"
side="$scratch/demo-worktrees/side-1"
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
for top in "$main" "$side"; do
    if [ "$top" = "$main" ]; then root="$BUILDS_ROOT/demo/main"; else root="$BUILDS_ROOT/demo/side-1"; fi
    mkdir -p "$root/target"
    printf '%s\n' "$top" > "$root/.checkout"
    ln -s "$root/target" "$top/target"
done
head -c 2097152 /dev/urandom > "$BUILDS_ROOT/demo/side-1/target/blob"
echo one > "$side/one" && git -C "$side" add one && git -C "$side" commit -q -m 'Add one'
echo two > "$side/two" && git -C "$side" add two && git -C "$side" commit -q -m 'Add two'
echo changed > "$side/one"
echo new > "$side/untracked"
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

# status --json, with and without the folders.
"$builds" status --json --no-folders > "$scratch/fast.json"
[ "$(json 'd["repos"][0]["name"]' < "$scratch/fast.json")" = '"demo"' ] || fail "no demo repo in $(cat "$scratch/fast.json")"
[ "$(json '[c["name"] for c in d["repos"][0]["checkouts"]]' < "$scratch/fast.json")" = '["main", "side-1"]' ] \
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

# The dashboard: its answers for the side worktree, the page, and a Host it
# doesn't serve.
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
"$builds" dashboard --port "$port" 2>"$scratch/dashboard.log" &
server=$!
for _ in $(seq 50); do
    curl -sf "http://127.0.0.1:$port/" >/dev/null 2>&1 && break
    sleep 0.1
done
curl -sf "http://127.0.0.1:$port/" | grep -q '<title>Builds</title>' || fail "no page: $(cat "$scratch/dashboard.log")"
curl -sf "http://127.0.0.1:$port/api/state" > "$scratch/state.json" || fail "no state"
[ "$(json '[x["subject"] for x in d["repos"][0]["checkouts"][1]["git"]["ahead"]]' < "$scratch/state.json")" = '["Add two", "Add one"]' ] \
    || fail "commits waiting to land: $(cat "$scratch/state.json")"
[ "$(json 'd["repos"][0]["checkouts"][1]["git"]["uncommitted"]' < "$scratch/state.json")" = 2 ] || fail "uncommitted count"
[ "$(json 'd["repos"][0]["checkouts"][1]["git"]["head"]["subject"]' < "$scratch/state.json")" = '"Add two"' ] || fail "HEAD"
[ "$(json 'd["repos"][0]["checkouts"][0]["git"]' < "$scratch/state.json")" = null ] || fail "asked git about main"
[ "$(json 'd["repos"][0]["checkouts"][1]["landing"]' < "$scratch/state.json")" = false ] || fail "landing without (landing)"
curl -sf "http://127.0.0.1:$port/api/folders" > "$scratch/folders.json" || fail "no folders"
[ "$(json 'd["repos"][0]["checkouts"][1]["total_kib"] >= 2048' < "$scratch/folders.json")" = true ] || fail "folders' size"
[ "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: example.com' "http://127.0.0.1:$port/api/state")" = 403 ] \
    || fail "served a foreign Host"
kill "$server"
wait "$server" 2>/dev/null || true
server=""

# Clearing the note keeps the claim; a landing claim is flagged.
"$builds" note "$side" 2>/dev/null
[ "$(cat "$claim_file")" = "agent A	$since" ] || fail "clearing wrote '$(cat "$claim_file")'"
"$builds" release "$side" 2>/dev/null
[ ! -e "$claim_file" ] || fail "release left the claim"
"$builds" claim "$side" "coordinator (landing)" 2>/dev/null
python3 -B -c 'import sys; sys.path.insert(0, sys.argv[1]); import dashboard; d = dashboard.state(sys.argv[2]); assert d["repos"][0]["checkouts"][1]["landing"], d' \
    "$script_dir" "$builds" || fail "landing claim not flagged"

echo "claims, notes, status --json and the dashboard: ok"

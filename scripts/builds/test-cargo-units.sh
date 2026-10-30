#!/usr/bin/env bash
# Tests `builds cargo` and the pruning of unused build copies (cargo_units.py)
# on a scratch cargo workspace in a temporary folder: nothing here touches the
# build volume.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
builds="$script_dir/builds"
units="$script_dir/cargo_units.py"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
ws="$scratch/ws"
profile="$ws/target/debug"

fail() { echo "FAIL: $*" >&2; exit 1; }

# A library with a build script, and a crate above it with a library that is
# also a staticlib and a cdylib, as bae-bridge is (its outputs carry no hash),
# a binary, an example, an integration test and a doctest.
mkdir -p "$ws/dep/src" "$ws/app/src/bin" "$ws/app/tests" "$ws/app/examples"
cat > "$ws/Cargo.toml" <<'EOF'
[workspace]
members = ["dep", "app"]
resolver = "2"
EOF
cat > "$ws/dep/Cargo.toml" <<'EOF'
[package]
name = "dep"
version = "0.1.0"
edition = "2021"
EOF
echo 'fn main() { println!("cargo:rerun-if-changed=build.rs"); }' > "$ws/dep/build.rs"
echo 'pub fn answer() -> u32 { 42 }' > "$ws/dep/src/lib.rs"
cat > "$ws/app/Cargo.toml" <<'EOF'
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
dep = { path = "../dep" }

[lib]
crate-type = ["lib", "staticlib", "cdylib"]
EOF
cat > "$ws/app/src/lib.rs" <<'EOF'
/// The answer.
///
/// ```
/// assert_eq!(app::answer(), 42);
/// ```
pub fn answer() -> u32 { dep::answer() }

#[cfg(test)]
mod tests {
    #[test]
    fn answer_is_42() { assert_eq!(super::answer(), 42); }
}
EOF
echo 'fn main() { println!("hello {}", app::answer()); }' > "$ws/app/src/bin/hello.rs"
echo '#[test] fn answer_through_the_crate() { assert_eq!(app::answer(), 42); }' > "$ws/app/tests/it.rs"
echo 'fn main() { println!("example {}", app::answer()); }' > "$ws/app/examples/ex.rs"

cd "$ws"

# Every build these tests use, through builds cargo or plain cargo ($1).
build_all() {
    "$@" build --all-targets -q
    "$@" doc --no-deps -q
    "$@" test -q -- answer > /dev/null
}

# Moves every time under the workspace back <days>, as if that long had
# passed since the last build, keeping each file's time relative to the others.
age() {
    python3 - "$1" "$ws" <<'PY'
import os, sys
delta = int(float(sys.argv[1]) * 86400 * 1e9)
for root, dirs, files in os.walk(sys.argv[2]):
    for path in [root] + [os.path.join(root, name) for name in dirs + files]:
        st = os.lstat(path)
        os.utime(path, ns=(st.st_atime_ns - delta, st.st_mtime_ns - delta), follow_symlinks=False)
PY
}

fingerprints() { find "$profile/.fingerprint" -mindepth 1 -maxdepth 1 "$@" | sort; }
recent() { fingerprints -newermt "$(date -v-1d '+%Y-%m-%d %H:%M')"; }

# Prints the units a cargo command ($@) rebuilt.
rebuilt() {
    cargo "$1" --message-format=json "${@:2}" 2>/dev/null \
        | python3 -c '
import json, sys
for line in sys.stdin:
    if line.startswith("{\"reason\":\"compiler-artifact\""):
        message = json.loads(line)
        if not message["fresh"]:
            print(message["target"]["name"])'
}

# builds cargo passes a program's output and its exit status through.
[ "$("$builds" cargo run -q --bin hello)" = "hello 42" ] || fail "cargo run's output did not pass through"
[ "$("$builds" cargo run -q --example ex)" = "example 42" ] || fail "an example's output did not pass through"
if "$builds" cargo build -q -p no-such-package 2> /dev/null; then
    fail "a failing cargo build exited 0 through builds cargo"
fi
if "$builds" cargo fmt 2> /dev/null; then
    fail "builds cargo ran a subcommand that builds nothing"
fi

# The build copies of the first version, then of a dependency bump.
build_all cargo
sed -i '' 's/^version = "0.1.0"/version = "0.1.1"/' dep/Cargo.toml
build_all cargo
all="$(fingerprints)"

# A build that finds everything up to date writes nothing cargo_units.py
# could read its use from.
age 5
build_all cargo
[ -z "$(recent)" ] || fail "an up-to-date build wrote in .fingerprint: $(recent)"

# builds cargo records the use of every unit the bumped build uses, rebuilt
# or not, and of none of the first version's.
build_all "$builds" cargo
used="$(recent)"
[ -n "$used" ] || fail "builds cargo recorded no use"
old="$(comm -23 <(echo "$all") <(echo "$used"))"
[ -n "$old" ] || fail "the dependency bump left no old build copies"
[ -z "$(rebuilt build --all-targets)" ] || fail "recording use made cargo rebuild"
incremental_before="$(ls "$profile/incremental" | wc -l)"

# A dry run deletes nothing.
python3 "$units" prune 3 1 "$ws/target" > /dev/null 2>&1
[ "$(fingerprints)" = "$all" ] || fail "a dry run deleted something"

# A build holding the profile folder's lock keeps everything.
python3 - "$profile/.cargo-lock" "$units" "$ws/target" <<'PY'
import fcntl, os, subprocess, sys
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT)
fcntl.flock(fd, fcntl.LOCK_EX)
result = subprocess.run(["python3", sys.argv[2], "prune", "3", "0", sys.argv[3]], capture_output=True, text=True)
assert "a build holds its lock" in result.stderr, result.stderr
PY
[ "$(fingerprints)" = "$all" ] || fail "pruning deleted while a build held the lock"

# A process holding a file of an old unit keeps that unit.
held_unit="$(echo "$old" | head -1)"
held_hash="${held_unit##*-}"
held_file="$(find "$profile/deps" -name "*-$held_hash*" | head -1)"
if [ -n "$held_file" ]; then
    exec 3< "$held_file"
fi

python3 "$units" prune 3 0 "$ws/target" > /dev/null
remaining="$(fingerprints)"
if [ -n "$held_file" ]; then
    exec 3<&-
    echo "$remaining" | grep -qxF "$held_unit" || fail "pruning deleted $held_unit while a process held $held_file"
    remaining="$(echo "$remaining" | grep -vxF "$held_unit")"
fi
[ "$remaining" = "$used" ] || fail "pruning kept $(comm -13 <(echo "$used") <(echo "$remaining")) and deleted $(comm -23 <(echo "$used") <(echo "$remaining"))"
for unit in $old; do
    [ "$unit" = "$held_unit" ] && [ -n "$held_file" ] && continue
    hash="${unit##*-}"
    leftover="$(find "$profile/deps" "$profile/build" "$profile/examples" -maxdepth 1 -name "*-$hash*")"
    [ -z "$leftover" ] || fail "pruning left files of $unit: $leftover"
done
[ "$(ls "$profile/incremental" | wc -l)" -lt "$incremental_before" ] || fail "pruning deleted no incremental folder"

# What the builds use is still up to date.
for command in "build --all-targets" "doc --no-deps" "test --no-run"; do
    # shellcheck disable=SC2086
    [ -z "$(rebuilt $command)" ] || fail "cargo $command rebuilt after pruning: $(rebuilt $command)"
done

echo "ok"

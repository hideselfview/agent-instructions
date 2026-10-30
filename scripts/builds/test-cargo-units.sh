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
# a binary, an example, an integration test and a doctest. Both compile
# incrementally, as path packages do by default, and keep their object files
# beside their outputs (split-debuginfo "unpacked", macOS's default).
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
cat > "$ws/dep/src/lib.rs" <<'EOF'
pub fn answer() -> u32 { 42 }

/// Panics on zero, from a frame in this rlib.
#[inline(never)]
pub fn nonzero(n: u32) -> u32 {
    if n == 0 {
        panic!("zero");
    }
    n
}
EOF
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

    #[test]
    fn panics_in_dep() { dep::nonzero(0); }
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

# A code change rebuilds the same units, each writing a new set of object
# files beside the ones it wrote before.
sed -i '' 's/dep::answer() }/dep::answer() + 0 }/' app/src/lib.rs
sed -i '' 's/n == 0 {/n == 0 \&\& n < 1 {/' dep/src/lib.rs
build_all "$builds" cargo
sets() { ls "$profile/deps" | grep "^$1[.-].*\.rcgu\.o$" | cut -d. -f1,3 | sort -u; }
[ "$(sets app | grep -c '^app\.')" -gt 1 ] || fail "rebuilding the cdylib left one set of its object files: $(sets app)"
[ -n "$(ls "$profile/deps" | grep '^dep-.*\.rcgu\.o$')" ] || fail "the dep rlib left no object files beside it"
used="$(recent)"
all="$(fingerprints)"
old="$(comm -23 <(echo "$all") <(echo "$used"))"

# A dry run deletes nothing.
python3 "$units" prune 3 12 1 "$ws/target" > /dev/null 2>&1
[ "$(fingerprints)" = "$all" ] || fail "a dry run deleted something"

# A build holding the profile folder's lock keeps everything.
python3 - "$profile/.cargo-lock" "$units" "$ws/target" <<'PY'
import fcntl, os, subprocess, sys
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT)
fcntl.flock(fd, fcntl.LOCK_EX)
result = subprocess.run(["python3", sys.argv[2], "prune", "3", "12", "0", sys.argv[3]], capture_output=True, text=True)
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

python3 "$units" prune 3 12 0 "$ws/target" > /dev/null
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

# Object files nothing reads are gone: an rlib's, which binaries read from
# its members, and every set but the newest, which the cdylib, the tests and
# the binaries were linked from.
dep_rlib_objects="$(for rlib in "$profile"/deps/libdep-*.rlib; do
    unit="$(basename "$rlib" .rlib)"; ls "$profile/deps" | grep "^${unit#lib}\..*\.rcgu\.o$" || true; done)"
[ -z "$dep_rlib_objects" ] || fail "pruning left object files beside the dep rlib: $dep_rlib_objects"
for owner in $(ls "$profile/deps" | grep '\.rcgu\.o$' | cut -d. -f1 | sort -u); do
    [ "$(sets "$owner" | grep -c "^$owner\.")" = 1 ] || fail "pruning left $owner more than one set of object files: $(sets "$owner")"
done
# Every object file a debug map names is still there.
for binary in $(find "$profile/deps" "$profile/examples" -maxdepth 1 -type f -perm -0100); do
    for object in $(nm -ap "$binary" 2>/dev/null | awk '$5 == "OSO" { print $6 }' | sed 's/(.*//'); do
        [ -e "$object" ] || fail "$(basename "$binary")'s debug map names $object, which pruning deleted"
    done
done
# A panic's backtrace still names file and line in the test and in the rlib.
trace="$(RUST_BACKTRACE=1 cargo test -q -p app --lib -- panics_in_dep 2>&1 || true)"
echo "$trace" | grep -A1 ' dep::nonzero$' | grep -q 'at .*dep/src/lib.rs:[0-9]' \
    || fail "the backtrace lost the dep rlib's file:line: $trace"
echo "$trace" | grep -A1 ' app::tests::panics_in_dep$' | grep -q 'at .*src/lib.rs:[0-9]' \
    || fail "the backtrace lost the test's file:line: $trace"

# What the builds use is still up to date.
for command in "build --all-targets" "doc --no-deps" "test --no-run"; do
    # shellcheck disable=SC2086
    [ -z "$(rebuilt $command)" ] || fail "cargo $command rebuilt after pruning: $(rebuilt $command)"
done

# A copy a newer one with the same identity replaced goes once nothing used
# it for the hours given, without waiting the days: a dependency bump leaves
# a copy of every unit above it, all of which the bump replaces.
age 0.6
# The copy a process held above is days old and goes with the first prune.
before="$(fingerprints | grep -vxF "$held_unit")"
sed -i '' 's/^version = "0.1.1"/version = "0.1.2"/' dep/Cargo.toml
build_all "$builds" cargo
bumped="$(recent)"
python3 "$units" prune 3 24 0 "$ws/target" > /dev/null 2>&1
[ "$(fingerprints)" = "$(sort <(echo "$before") <(echo "$bumped") | uniq)" ] \
    || fail "pruning deleted copies used within the last 24 hours: $(comm -23 <(sort <(echo "$before") <(echo "$bumped") | uniq) <(fingerprints))"
python3 "$units" prune 3 12 0 "$ws/target" > /dev/null 2>&1
[ "$(fingerprints)" = "$bumped" ] \
    || fail "pruning kept $(comm -13 <(echo "$bumped") <(fingerprints)) and deleted $(comm -23 <(echo "$bumped") <(fingerprints))"
for command in "build --all-targets" "doc --no-deps" "test --no-run"; do
    # shellcheck disable=SC2086
    [ -z "$(rebuilt $command)" ] || fail "cargo $command rebuilt after deleting replaced copies: $(rebuilt $command)"
done

# Cargo's git checkouts no Cargo.lock pins go; pinned ones, and the git
# databases, stay. A cargo holding the package cache keeps them all.
home="$scratch/cargo-home"
pinned=04561fe49ad0d5646b41dd3d213d029f566035c7
mkdir -p "$home/git/checkouts/coven-f7c984c541c70e3c/04561fe" "$home/git/checkouts/coven-f7c984c541c70e3c/c24cf1d" \
    "$home/git/checkouts/other-2dad6eb5752945c7/c0649fc" "$home/git/db/coven-f7c984c541c70e3c"
printf '[[package]]\nname = "coven"\nsource = "git+https://github.com/bae-fm/coven?rev=%s#%s"\n' "$pinned" "$pinned" > "$scratch/Cargo.lock"
checkouts() { (cd "$home/git" && { find checkouts -mindepth 2 -maxdepth 2; find db -mindepth 1 -maxdepth 1; } | sort); }
python3 - "$home/.package-cache" "$units" "$home" "$scratch/Cargo.lock" <<'PY'
import fcntl, os, subprocess, sys
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT)
fcntl.flock(fd, fcntl.LOCK_EX)
result = subprocess.run(["python3", sys.argv[2], "git-checkouts", sys.argv[3], "0", sys.argv[4]], capture_output=True, text=True)
assert "a cargo holds its package cache" in result.stderr, result.stderr
PY
[ "$(checkouts | wc -l)" -eq 4 ] || fail "pruning git checkouts deleted while a cargo held the package cache: $(checkouts)"
python3 "$units" git-checkouts "$home" 0 "$scratch/Cargo.lock" > /dev/null 2>&1
[ "$(checkouts)" = "$(printf '%s\n' checkouts/coven-f7c984c541c70e3c/04561fe db/coven-f7c984c541c70e3c)" ] \
    || fail "pruning git checkouts left $(checkouts)"

echo "ok"

#!/usr/bin/env bash
# Tests `builds cargo` and the pruning of unused build copies (`builds units`)
# on a scratch cargo workspace in a temporary folder: nothing here touches the
# build volume.

set -euo pipefail

# cargo_units.rs runs this with BUILDS_BIN set to the built `builds`, TOOLCHAIN
# set to the Rust toolchain every cargo here runs, and LAYOUT set to the
# layout that toolchain's cargo writes: shared (.fingerprint beside deps,
# cargo before 1.100) or unit-folders (build/<package>/<hash>/, cargo 1.100
# on). OLDER_TOOLCHAIN, when set, names a toolchain whose cargo writes shared
# folders, for the steps on a profile folder that holds both layouts.
builds="${BUILDS_BIN:?run through cargo test --test cargo_units}"
export RUSTUP_TOOLCHAIN="${TOOLCHAIN:?run through cargo test --test cargo_units}"
layout="${LAYOUT:?run through cargo test --test cargo_units}"
export UNITS_CMD="$builds units"
# The steps setup and prune run on cargo's folders, on the folders named.
units() { $UNITS_CMD "$@"; }
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
# The workspace's Cargo.lock, as a repo commits it: builds cargo runs cargo
# with --locked.
cargo generate-lockfile -q
# builds cargo has cargo link with builds-linker (linker.rs), and cargo
# rebuilds every unit it links when the linker changes; so the workspace
# links with it too, and a plain cargo build is up to date after a builds
# cargo build of the same sources.
mkdir .cargo
printf '[target.%s]\nlinker = "%s"\n' "$(rustc -vV | sed -n 's/^host: //p')" "$HOME/.local/bin/builds-linker" > .cargo/config.toml

# Bumps dep's version from $1 to $2 and updates Cargo.lock to match, as the
# commit that bumps it would.
bump() {
    sed -i '' "s/^version = \"$1\"/version = \"$2\"/" dep/Cargo.toml
    cargo update -q -w
}

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

# The fingerprint folder of every unit in the profile folder, in either
# layout: .fingerprint/<package>-<hash> or build/<package>/<hash>/fingerprint.
fingerprints() {
    {
        [ ! -d "$profile/.fingerprint" ] || find "$profile/.fingerprint" -mindepth 1 -maxdepth 1 "$@"
        [ ! -d "$profile/build" ] || find "$profile/build" -mindepth 3 -maxdepth 3 -type d -name fingerprint "$@"
    } | sort
}
recent() { fingerprints -newermt "$(date -v-1d '+%Y-%m-%d %H:%M')"; }
# The hash of the unit whose fingerprint folder is $1.
hash_of() {
    case "$1" in
        */fingerprint) basename "$(dirname "$1")" ;;
        *) echo "${1##*-}" ;;
    esac
}
# The files of the unit whose fingerprint folder is $1, besides that folder:
# those named by its hash in deps, build and examples, or its own folder's.
unit_files() {
    case "$1" in
        */fingerprint) [ ! -d "$(dirname "$1")" ] || find "$(dirname "$1")" -mindepth 1 -type f -not -path "$1/*" ;;
        *) find "$profile/deps" "$profile/build" "$profile/examples" -maxdepth 1 -name "*-$(hash_of "$1")*" ;;
    esac
}
# The first regular file among them, which a test can hold open.
unit_file() {
    local path first=""
    while read -r path; do
        if [ -z "$first" ] && [ -f "$path" ]; then first="$path"; fi
    done < <(unit_files "$1")
    echo "$first"
}
# The folders rustc writes units' outputs and object files in.
output_dirs() {
    if [ "$layout" = shared ]; then
        echo "$profile/deps"
        echo "$profile/examples"
    else
        find "$profile/build" -mindepth 3 -maxdepth 3 -type d -name out
    fi
}
# The test binary of the unit whose fingerprint folder is $1.
test_binary() {
    case "$1" in
        */fingerprint) echo "$(dirname "$1")/out/app-$(hash_of "$1")" ;;
        *) echo "$profile/deps/app-$(hash_of "$1")" ;;
    esac
}

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
    fail "builds cargo ran a subcommand that neither builds nor only reads the workspace"
fi

# builds cargo runs the subcommands that only read the workspace, and passes
# their output through.
"$builds" cargo metadata --no-deps --format-version 1 | grep -q '"name":"app"' \
    || fail "cargo metadata's output did not pass through"
"$builds" cargo tree -q -p app | grep -q '── dep v0.1.0' || fail "cargo tree's output did not pass through"

# builds cargo fails on a Cargo.lock that disagrees with the manifests, and
# leaves the lock as the commit has it, for a read as for a build.
cp Cargo.lock "$scratch/Cargo.lock"
sed -i '' 's/^version = "0.1.0"/version = "0.1.9"/' dep/Cargo.toml
if "$builds" cargo check -q 2> /dev/null; then
    fail "builds cargo built with a Cargo.lock that disagrees with the manifests"
fi
cmp -s Cargo.lock "$scratch/Cargo.lock" || fail "builds cargo changed a stale Cargo.lock"
if "$builds" cargo tree -q 2> /dev/null; then
    fail "builds cargo read the workspace with a Cargo.lock that disagrees with the manifests"
fi
cmp -s Cargo.lock "$scratch/Cargo.lock" || fail "a read through builds cargo changed a stale Cargo.lock"
sed -i '' 's/^version = "0.1.9"/version = "0.1.0"/' dep/Cargo.toml

# The build copies of the first version, then of a dependency bump.
build_all cargo
case "$layout" in
    shared) [ -d "$profile/.fingerprint" ] && [ ! -d "$profile/build/dep" ] \
        || fail "$TOOLCHAIN's cargo did not write shared folders: $(ls "$profile")" ;;
    unit-folders) [ ! -e "$profile/.fingerprint" ] && [ -n "$(fingerprints)" ] \
        || fail "$TOOLCHAIN's cargo did not write unit folders: $(ls "$profile")" ;;
    *) fail "LAYOUT is $layout, not shared or unit-folders" ;;
esac
bump 0.1.0 0.1.1
build_all cargo
all="$(fingerprints)"

# A build that finds everything up to date writes nothing `builds units`
# could read its use from. Cargo 1.100 records the newest time of a path
# package's files in its documentation's fingerprint, so after aging moves
# those times it documents the package again, once.
age 5
build_all cargo
since="$scratch/since"
touch "$since"
build_all cargo
written="$(fingerprints -newer "$since")"
[ -z "$written" ] || fail "an up-to-date build wrote in a fingerprint folder: $(for dir in $written; do ls -la "$dir"; done)"

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
# Every object file in the output folders, as <folder>/<owner> <set>: the
# owner is the part of its name before the first dot (a crate, or a crate and
# its unit's hash).
objects() {
    output_dirs | while read -r dir; do
        find "$dir" -maxdepth 1 -name '*.rcgu.o' | while read -r path; do
            name="$(basename "$path")"
            echo "$dir/${name%%.*} $(echo "$name" | cut -d. -f3)"
        done
    done | sort -u
}
sets() { objects | grep "/$1 " || true; }
[ -n "$(sets app | cut -d' ' -f1 | uniq -d)" ] || fail "rebuilding the cdylib left one set of its object files: $(sets app)"
objects | grep -q '/dep-[0-9a-f]* ' || fail "the dep rlib left no object files beside it"
used="$(recent)"
all="$(fingerprints)"
old="$(comm -23 <(echo "$all") <(echo "$used"))"

# A dry run deletes nothing.
units prune 3 100000 1 "$ws/target" > /dev/null 2>&1
[ "$(fingerprints)" = "$all" ] || fail "a dry run deleted something"

# A build holding the profile folder's lock keeps everything.
python3 - "$profile/.cargo-lock" - "$ws/target" <<'PY'
import fcntl, os, shlex, subprocess, sys
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT)
fcntl.flock(fd, fcntl.LOCK_EX)
result = subprocess.run(shlex.split(os.environ["UNITS_CMD"]) + ["prune", "3", "100000", "0", sys.argv[3]], capture_output=True, text=True)
assert "a build holds its lock" in result.stderr, result.stderr
PY
[ "$(fingerprints)" = "$all" ] || fail "pruning deleted while a build held the lock"

# A process holding a file of an old unit keeps that unit.
held_unit="${old%%$'\n'*}"
held_file="$(unit_file "$held_unit")"
if [ -n "$held_file" ]; then
    exec 3< "$held_file"
fi

units prune 3 100000 0 "$ws/target" > /dev/null
remaining="$(fingerprints)"
if [ -n "$held_file" ]; then
    exec 3<&-
    grep -qxF "$held_unit" <<< "$remaining" || fail "pruning deleted $held_unit while a process held $held_file"
    remaining="$(echo "$remaining" | grep -vxF "$held_unit")"
fi
[ "$remaining" = "$used" ] || fail "pruning kept $(comm -13 <(echo "$used") <(echo "$remaining")) and deleted $(comm -23 <(echo "$used") <(echo "$remaining"))"
for unit in $old; do
    [ "$unit" = "$held_unit" ] && [ -n "$held_file" ] && continue
    leftover="$(unit_files "$unit")"
    [ -z "$leftover" ] || fail "pruning left files of $unit: $leftover"
    case "$unit" in
        */fingerprint) [ ! -e "$(dirname "$unit")" ] || fail "pruning left the folder of $unit" ;;
    esac
done
[ "$(ls "$profile/incremental" | wc -l)" -lt "$incremental_before" ] || fail "pruning deleted no incremental folder"

# Object files nothing reads are gone: an rlib's, which binaries read from
# its members, and every set but the newest, which the cdylib, the tests and
# the binaries were linked from.
dep_rlib_objects="$(output_dirs | while read -r dir; do
    for rlib in "$dir"/libdep-*.rlib; do
        [ -e "$rlib" ] || continue
        unit="$(basename "$rlib" .rlib)"
        find "$dir" -maxdepth 1 -name "${unit#lib}.*.rcgu.o"
    done
done)"
[ -z "$dep_rlib_objects" ] || fail "pruning left object files beside the dep rlib: $dep_rlib_objects"
for owner in $(objects | cut -d' ' -f1 | sort -u); do
    [ "$(objects | grep -c "^$owner ")" = 1 ] || fail "pruning left $owner more than one set of object files: $(objects | grep "^$owner ")"
done
# Every object file a debug map names is still there.
while read -r binary; do
    for object in $(nm -ap "$binary" 2>/dev/null | awk '$5 == "OSO" { print $6 }' | sed 's/(.*//'); do
        [ -e "$object" ] || fail "$(basename "$binary")'s debug map names $object, which pruning deleted"
    done
done < <(output_dirs | while read -r dir; do find "$dir" -maxdepth 1 -type f -perm -0100; done)
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

# A copy a newer one built from other sources replaced goes at the next
# prune, within days of its last use: a dependency bump leaves a copy of every
# unit above it, all of which the bump replaces.
age 0.6
bump 0.1.1 0.1.2
build_all "$builds" cargo
bumped="$(recent)"
units prune 3 100000 0 "$ws/target" > /dev/null 2>&1
[ "$(fingerprints)" = "$bumped" ] \
    || fail "pruning kept $(comm -13 <(echo "$bumped") <(fingerprints)) and deleted $(comm -23 <(echo "$bumped") <(fingerprints))"
for command in "build --all-targets" "doc --no-deps" "test --no-run"; do
    # shellcheck disable=SC2086
    [ -z "$(rebuilt $command)" ] || fail "cargo $command rebuilt after deleting replaced copies: $(rebuilt $command)"
done

# While a `builds cargo` build holds its checkout's guard (cargo test holds
# no cargo lock while it runs the tests), nothing is deleted.
age 1.1
bump 0.1.2 0.1.3
build_all "$builds" cargo
replaced_copies="$(fingerprints)"
touch "$ws/.builds-cargo.lock"
python3 - "$ws/.builds-cargo.lock" - "$ws/target" <<'PY'
import fcntl, os, shlex, subprocess, sys
fd = os.open(sys.argv[1], os.O_RDWR)
fcntl.flock(fd, fcntl.LOCK_SH)
result = subprocess.run(shlex.split(os.environ["UNITS_CMD"]) + ["prune", "3", "100000", "0", sys.argv[3]], capture_output=True, text=True)
assert "a build is running in its checkout" in result.stderr, result.stderr
PY
[ "$(fingerprints)" = "$replaced_copies" ] || fail "pruning deleted while a build held the checkout's guard"

# A process holding files in the folder (the current test binary, running)
# doesn't keep the replaced copies; one holding a replaced copy's file keeps
# that copy.
current="$(recent)"
replaced_now="$(comm -23 <(echo "$replaced_copies") <(echo "$current"))"
[ -n "$replaced_now" ] || fail "the bump left no replaced copies"
current_test="$(echo "$current" | while read -r dir; do
    if [ -e "$dir/test-lib-app" ]; then test_binary "$dir"; break; fi; done)"
[ -x "$current_test" ] || fail "found no test binary of the current copy"
# Held open, as a running test binary holds its file.
exec 7< "$current_test"
kept_unit="$(echo "$replaced_now" | while read -r dir; do
    if [ -n "$(unit_file "$dir")" ]; then echo "$dir"; break; fi; done)"
[ -n "$kept_unit" ] || fail "found no replaced copy with an output file to hold"
exec 8< "$(unit_file "$kept_unit")"
prune_out="$(units prune 3 100000 0 "$ws/target" 2>&1 >/dev/null)"
exec 7<&- 8<&-
case "$prune_out" in
    *"a later build from other sources replaced"*) ;;
    *) fail "pruning beside a held file deleted no replaced copy: $prune_out" ;;
esac
case "$prune_out" in
    *"kept 1 build copy in"*"a process holds their files"*) ;;
    *) fail "pruning did not keep the held copy: $prune_out" ;;
esac
[ "$(fingerprints)" = "$(sort <(echo "$current") <(echo "$kept_unit") | uniq)" ] \
    || fail "pruning beside held files left $(comm -13 <(echo "$current") <(fingerprints)) and deleted $(comm -23 <(echo "$current") <(fingerprints))"
[ -x "$current_test" ] || fail "pruning deleted the current test binary a process held"
for command in "build --all-targets" "doc --no-deps" "test --no-run"; do
    # shellcheck disable=SC2086
    [ -z "$(rebuilt $command)" ] || fail "cargo $command rebuilt after pruning beside held files: $(rebuilt $command)"
done

# In a checkout whose builds run without incremental compilation (0 hours),
# every incremental folder goes, and the builds stay up to date.
[ -n "$(ls "$profile/incremental")" ] || fail "the builds wrote no incremental folders"
units prune 3 0 0 "$ws/target" > /dev/null 2>&1
[ -z "$(ls "$profile/incremental")" ] || fail "pruning for a checkout without incremental left $(ls "$profile/incremental")"
for command in "build --all-targets" "doc --no-deps" "test --no-run"; do
    # shellcheck disable=SC2086
    [ -z "$(rebuilt $command)" ] || fail "cargo $command rebuilt after deleting the incremental folders: $(rebuilt $command)"
done

# Copies built from the same sources against other features of a
# dependency (another package selection) stay side by side: builds use them
# in turn.
printf '\n[features]\nextra = []\n' >> dep/Cargo.toml
build_all "$builds" cargo
units prune 3 100000 0 "$ws/target" > /dev/null 2>&1
# The fingerprint folders of dep's library: one with the feature, one
# without.
lib_dep() { fingerprints | while read -r dir; do if [ -e "$dir/lib-dep" ]; then echo "$dir"; fi; done; }
has_extra() { grep -qF '"features":"[\"extra\"]"' "$1/lib-dep.json"; }
with_extra() { for dir in $(lib_dep); do if has_extra "$dir"; then echo "$dir"; fi; done; }
"$builds" cargo build -q -p app --lib --features dep/extra
"$builds" cargo build -q -p app --lib
[ -n "$(with_extra)" ] && [ -n "$(comm -23 <(lib_dep) <(with_extra))" ] \
    || fail "the feature variant built no second copy of dep's library: $(lib_dep)"
variants="$(fingerprints)"
units prune 3 100000 0 "$ws/target" > /dev/null 2>&1
[ "$(fingerprints)" = "$variants" ] \
    || fail "pruning deleted a feature variant: $(comm -23 <(echo "$variants") <(fingerprints))"

# In a checkout with a build root, `builds cargo` holds the checkout's guard
# while cargo runs and, as it exits, starts the prune that deletes what its
# build replaced; that prune holds a lock on its log until it ends.
age 1.1
volume="$scratch/volume"
mkdir -p "$volume/ws/main"
git -C "$ws" init -q
echo "target 1G" > "$ws/.build-dirs"
mv "$ws/target" "$volume/ws/main/target"
ln -s "$volume/ws/main/target" "$ws/target"
bump 0.1.3 0.1.4
[ -z "$(recent)" ] || fail "aging left recent copies: $(recent)"
BUILDS_ROOT="$volume" "$builds" cargo build -q --all-targets
[ -f "$volume/ws/main/.builds-cargo.lock" ] || fail "builds cargo took no guard in the checkout's build root"
lockf -k -t 60 "$volume/ws/main/.after-build.log" true || fail "the prune after the build ran for over a minute"
bumped_libs="$(comm -12 <(lib_dep) <(recent))"
[ -n "$bumped_libs" ] || fail "the bumped build left no library copy of dep"
[ "$(lib_dep)" = "$(sort <(echo "$bumped_libs") <(comm -23 <(lib_dep) <(recent) | while read -r dir; do if grep -q extra "$dir/lib-dep.json"; then echo "$dir"; fi; done) | uniq)" ] \
    || fail "the prune after the build left $(comm -13 <(echo "$bumped_libs") <(lib_dep)): $(cat "$volume/ws/main/.after-build.log")"
case "$(cat "$volume/ws/main/.after-build.log")" in
    *"a later build from other sources replaced"*) ;;
    *) fail "the prune after the build said: $(cat "$volume/ws/main/.after-build.log")" ;;
esac
rm -f "$ws/target"
mv "$volume/ws/main/target" "$ws/target"

# Cargo's git checkouts no Cargo.lock pins go; pinned ones, and the git
# databases, stay. A cargo holding the package cache keeps them all.
home="$scratch/cargo-home"
pinned=04561fe49ad0d5646b41dd3d213d029f566035c7
mkdir -p "$home/git/checkouts/coven-f7c984c541c70e3c/04561fe" "$home/git/checkouts/coven-f7c984c541c70e3c/c24cf1d" \
    "$home/git/checkouts/other-2dad6eb5752945c7/c0649fc" "$home/git/db/coven-f7c984c541c70e3c"
printf '[[package]]\nname = "coven"\nsource = "git+https://github.com/bae-fm/coven?rev=%s#%s"\n' "$pinned" "$pinned" > "$scratch/Cargo.lock"
checkouts() { (cd "$home/git" && { find checkouts -mindepth 2 -maxdepth 2; find db -mindepth 1 -maxdepth 1; } | sort); }
python3 - "$home/.package-cache" - "$home" "$scratch/Cargo.lock" <<'PY'
import fcntl, os, shlex, subprocess, sys
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT)
fcntl.flock(fd, fcntl.LOCK_EX)
result = subprocess.run(shlex.split(os.environ["UNITS_CMD"]) + ["git-checkouts", sys.argv[3], "0", sys.argv[4]], capture_output=True, text=True)
assert "a cargo holds its package cache" in result.stderr, result.stderr
PY
[ "$(checkouts | wc -l)" -eq 4 ] || fail "pruning git checkouts deleted while a cargo held the package cache: $(checkouts)"
units git-checkouts "$home" 0 "$scratch/Cargo.lock" > /dev/null 2>&1
[ "$(checkouts)" = "$(printf '%s\n' checkouts/coven-f7c984c541c70e3c/04561fe db/coven-f7c984c541c70e3c)" ] \
    || fail "pruning git checkouts left $(checkouts)"

# Incremental folders nothing wrote anywhere inside for the idle hours go, in
# every profile folder; one with a single fresh file deep inside, one a process
# holds a file of, and everything while a build holds the profile's lock stay.
# A dry run deletes nothing.
idle_target="$scratch/idle/target"
idle_profile="$idle_target/debug"
for dir in "$idle_profile" "$idle_target/release"; do
    if [ "$layout" = shared ]; then
        mkdir -p "$dir/.fingerprint" "$dir/deps"
    else
        mkdir -p "$dir/build/dep/0123456789abcdef/fingerprint"
    fi
done
for n in stale fresh deep held; do
    for dir in "$idle_profile" "$idle_target/release"; do
        mkdir -p "$dir/incremental/$n-id/s-1-abc"
        head -c 262144 /dev/urandom > "$dir/incremental/$n-id/s-1-abc/data"
    done
done
old() { touch -t "$(date -v-7H +%Y%m%d%H%M)" "$@"; }
for dir in "$idle_profile" "$idle_target/release"; do
    for n in stale deep held; do
        old "$dir/incremental/$n-id/s-1-abc/data" "$dir/incremental/$n-id/s-1-abc" "$dir/incremental/$n-id"
    done
done
# deep: everything old but a file inside, written now.
touch "$idle_profile/incremental/deep-id/s-1-abc/data"
idle_left() { ls "$idle_profile/incremental" | tr '\n' ' '; }
release_left() { ls "$idle_target/release/incremental" | tr '\n' ' '; }
dry_out="$(units prune 3 6 1 "$idle_target" 2>&1 >/dev/null)"
[ "$(idle_left)" = "deep-id fresh-id held-id stale-id " ] || fail "a dry run deleted idle incremental folders: $(idle_left)"
case "$dry_out" in
    *"would delete 2 incremental folders nothing wrote to in 6 hours from"*"debug"*) ;;
    *) fail "the dry run did not report the idle folders: $dry_out" ;;
esac
python3 - "$idle_profile/.cargo-lock" - "$idle_target" <<'PY'
import fcntl, os, shlex, subprocess, sys
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT)
fcntl.flock(fd, fcntl.LOCK_EX)
result = subprocess.run(shlex.split(os.environ["UNITS_CMD"]) + ["prune", "3", "6", "0", sys.argv[3]], capture_output=True, text=True)
assert "kept 2 incremental folders in" in result.stderr and "(nothing wrote to in 6 hours): a build holds its lock" in result.stderr, result.stderr
PY
[ "$(idle_left)" = "deep-id fresh-id held-id stale-id " ] || fail "deleted idle folders while a build held the lock: $(idle_left)"
[ "$(release_left)" = "fresh-id " ] || fail "the release profile's idle folders survived: $(release_left)"
exec 6< "$idle_profile/incremental/held-id/s-1-abc/data"
live_out="$(units prune 3 6 0 "$idle_target" 2>&1 >/dev/null)"
exec 6<&-
[ "$(idle_left)" = "deep-id fresh-id held-id " ] || fail "idle pruning left the wrong folders: $(idle_left)"
case "$live_out" in
    *"deleted 1 incremental folder nothing wrote to in 6 hours from"*) ;;
    *) fail "pruning did not report the idle folder: $live_out" ;;
esac

# A profile folder that holds both layouts, as a checkout's does once it
# moves from a cargo that writes shared folders to one that writes unit
# folders: builds cargo records the newer cargo's units, and a prune deletes
# the older cargo's units once no build has used them for the days, and keeps
# every unit the newer cargo's builds use.
if [ -n "${OLDER_TOOLCHAIN:-}" ]; then
    ws="$scratch/mixed"
    profile="$ws/target/debug"
    mkdir -p "$ws"
    cp -R "$scratch/ws/Cargo.toml" "$scratch/ws/Cargo.lock" "$scratch/ws/.cargo" "$scratch/ws/dep" "$scratch/ws/app" "$ws/"
    cd "$ws"
    RUSTUP_TOOLCHAIN="$OLDER_TOOLCHAIN" "$builds" cargo build -q --all-targets
    [ -d "$profile/.fingerprint" ] || fail "$OLDER_TOOLCHAIN's cargo wrote no shared folders: $(ls "$profile")"
    older="$(fingerprints)"
    age 4
    "$builds" cargo build -q --all-targets
    newer="$(recent)"
    [ -n "$(echo "$newer" | grep '/fingerprint$')" ] || fail "$TOOLCHAIN's cargo wrote no unit folders beside the shared ones"
    [ "$(sort <(echo "$older") <(echo "$newer") | uniq)" = "$(fingerprints)" ] \
        || fail "builds cargo recorded the use of none of $(comm -23 <(fingerprints) <(sort <(echo "$older") <(echo "$newer") | uniq))"
    units prune 3 100000 0 "$ws/target" > /dev/null 2>&1
    [ "$(fingerprints)" = "$newer" ] \
        || fail "pruning both layouts kept $(comm -13 <(echo "$newer") <(fingerprints)) and deleted $(comm -23 <(echo "$newer") <(fingerprints))"
    for unit in $older; do
        leftover="$(unit_files "$unit")"
        [ -z "$leftover" ] || fail "pruning left files of $unit: $leftover"
    done
    [ -z "$(rebuilt build --all-targets)" ] || fail "cargo build rebuilt after pruning both layouts: $(rebuilt build --all-targets)"
fi

echo "ok"

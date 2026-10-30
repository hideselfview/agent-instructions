#!/usr/bin/env python3
"""Cargo's build copies in a build folder: recording which ones a build used,
and deleting the ones no build used for a while. `builds` runs this for its
`cargo`, `record-use`, `setup` and `prune` commands.

Cargo builds each crate target as a unit and names the unit's files after a
hash of everything that build depends on, dependency versions included, so a
dependency bump builds a new copy of every unit above it and the old copies
stay. In each profile folder (<target>/debug, <target>/<triple>/release, and a
nested target such as a `cargo install --target-dir` one) a unit owns

    .fingerprint/<package>-<hash>/   cargo's record that the unit is built
    deps/<crate>-<hash>*, deps/lib<crate>-<hash>*, examples/<name>-<hash>*
    build/<package>-<hash>/          a build script, or what running it wrote
    incremental/<crate>-<id>/        rustc's incremental state

A cdylib or staticlib unit's files in deps carry no hash: each build of that
crate overwrites them, so they never pile up and nothing here deletes them.
The incremental folder's id is rustc's, not cargo's hash; it belongs to the
unit whose deps files are hard links to its codegen and metadata files.

A build that finds a unit up to date writes nothing in any of these, so its
use is recorded from cargo's own report (`--message-format=json-...`): `record`
sets the modification time of the unit's fingerprint folder to now. A unit's
last use is the newest modification time of that folder and the files in it,
which for a unit no recorded build has used is when cargo last built it.
"""

import errno
import fcntl
import json
import os
import re
import shutil
import subprocess
import sys
import time

# deps/libbae_core-8fffeaeaf85dc425.rlib, deps/bae_core-8fffeaeaf85dc425 (a
# test), deps/bae_core-8fffeaeaf85dc425.bae_core.1c1aebe005fd4a7f-cgu.00.rcgu.o
HASHED_FILE = re.compile(r"^(?:lib)?(?P<crate>[A-Za-z0-9_]+?)-(?P<hash>[0-9a-f]{16})(?:\..+)?$")
# .fingerprint/bae-core-8fffeaeaf85dc425, build/aws-lc-sys-3de0343d4993181b
UNIT_DIR = re.compile(r"^(?P<package>.+)-(?P<hash>[0-9a-f]{16})$")
# Folders a profile folder keeps per-unit files in, beside .fingerprint.
OUTPUT_DIRS = ("deps", "examples")
# How deep below a build folder a profile folder can sit:
# owner-construction-check/<rev>/release.
PROFILE_DEPTH = 3


def note(message):
    print(f"builds: {message}", file=sys.stderr)


def counted(count, one, many):
    return f"{count} {one if count == 1 else many}"


def human(size):
    if size >= 1 << 30:
        return f"{size / (1 << 30):.1f}G"
    return f"{size // (1 << 20)}M"


def is_profile_dir(path):
    return os.path.isdir(os.path.join(path, ".fingerprint"))


def profile_dirs(folder):
    """Every profile folder in <folder>, at most PROFILE_DEPTH levels down."""
    found = []

    def walk(path, depth):
        try:
            entries = list(os.scandir(path))
        except (FileNotFoundError, NotADirectoryError, PermissionError):
            return
        if any(entry.name == ".fingerprint" and entry.is_dir(follow_symlinks=False) for entry in entries):
            found.append(path)
            return
        if depth == PROFILE_DEPTH:
            return
        for entry in entries:
            if entry.is_dir(follow_symlinks=False) and entry.name != "doc":
                walk(entry.path, depth + 1)

    walk(folder, 0)
    return found


# ── Recording use ───────────────────────────────────────────────────────────


class Fingerprints:
    """The fingerprint folders of each profile folder, by unit hash. A build
    adds units while it reports them, so a hash not yet listed lists again."""

    def __init__(self):
        self.listed = {}

    def _list(self, profile):
        units = {}
        for name in os.listdir(os.path.join(profile, ".fingerprint")):
            match = UNIT_DIR.match(name)
            if match:
                units[match["hash"]] = name
        self.listed[profile] = units
        return units

    def get(self, profile, unit_hash):
        units = self.listed.get(profile)
        if units is None or unit_hash not in units:
            units = self._list(profile)
        name = units.get(unit_hash)
        return os.path.join(profile, ".fingerprint", name) if name else None

    def by_package(self, profile, package):
        units = self.listed.get(profile) or self._list(profile)
        prefix = package + "-"
        return [
            os.path.join(profile, ".fingerprint", name)
            for name in units.values()
            if name.startswith(prefix) and UNIT_DIR.match(name)["package"] == package
        ]


def hashed_unit(path):
    """(<profile folder>, <hash>) of a path cargo reported, when the path names
    its unit's hash: deps/<crate>-<hash>*, examples/<name>-<hash>*, or a file
    in build/<package>-<hash>/; None otherwise."""
    parent = os.path.dirname(path)
    if os.path.basename(parent) in OUTPUT_DIRS:
        match = HASHED_FILE.match(os.path.basename(path))
        if match and is_profile_dir(os.path.dirname(parent)):
            return os.path.dirname(parent), match["hash"]
    while parent not in ("", os.sep):
        above = os.path.dirname(parent)
        if os.path.basename(above) == "build":
            match = UNIT_DIR.match(os.path.basename(parent))
            if match and is_profile_dir(os.path.dirname(above)):
                return os.path.dirname(above), match["hash"]
        parent = above
    return None


def copied_from(path):
    """The (<profile folder>, <hash>) of each unit whose deps (or examples)
    file cargo copied to <path>, a target cargo put beside them
    (target/debug/loc-gen for deps/loc_gen-<hash>): the copy keeps the
    original's size and modification time, which no two builds share."""
    parent = os.path.dirname(path)
    if is_profile_dir(parent):
        profile, outputs = parent, os.path.join(parent, "deps")
    elif os.path.basename(parent) == "examples" and is_profile_dir(os.path.dirname(parent)):
        profile, outputs = os.path.dirname(parent), parent
    else:
        return []
    try:
        copy = os.stat(path)
        names = os.listdir(outputs)
    except FileNotFoundError:
        return []
    found = []
    for name in names:
        match = HASHED_FILE.match(name)
        if not match:
            continue
        try:
            original = os.stat(os.path.join(outputs, name))
        except FileNotFoundError:
            continue
        if original.st_size == copy.st_size and original.st_mtime_ns == copy.st_mtime_ns:
            found.append((profile, match["hash"]))
    return found


def package_name(package_id):
    """The package name in a cargo package id: registry+https://...#serde@1.0.228,
    path+file:///.../bae-loc#0.1.0 (the folder's name when it is the package's)."""
    location, _, version = package_id.rpartition("#")
    if "@" in version:
        return version.split("@", 1)[0]
    return location.split("?", 1)[0].rstrip("/").rsplit("/", 1)[-1]


TARGET_KINDS = {
    "lib": "lib", "rlib": "lib", "dylib": "lib", "cdylib": "lib", "staticlib": "lib",
    "proc-macro": "lib", "bin": "bin", "example": "example", "test": "integration-test",
    "bench": "bench", "custom-build": "build-script",
}


def fingerprint_file(message, doc):
    """The name cargo gives a unit's fingerprint file: doc-lib-bae_core,
    bin-loc-gen, test-lib-bae_core."""
    target = message["target"]
    flavor = "doc-" if doc else "test-" if message["profile"].get("test") else ""
    return f"{flavor}{TARGET_KINDS.get(target['kind'][0], target['kind'][0])}-{target['name']}"


def fingerprint_values(fingerprint_dir):
    """The fingerprints in a unit's fingerprint folder: each file holding one
    names it in hex, of the number's little-endian bytes, the form a
    dependent's JSON lists its dependencies' fingerprints in as numbers."""
    values = set()
    try:
        names = os.listdir(fingerprint_dir)
    except FileNotFoundError:
        return values
    for name in names:
        if name.endswith(".json") or name.startswith(("dep-", "output-")) or name == "invoked.timestamp":
            continue
        try:
            with open(os.path.join(fingerprint_dir, name)) as f:
                values.add(int.from_bytes(bytes.fromhex(f.read().strip()), "little"))
        except (OSError, ValueError):
            pass
    return values


def recorded(fingerprint_dir, file_name):
    """The JSON cargo keeps beside a unit's fingerprint file; {} when unreadable."""
    try:
        with open(os.path.join(fingerprint_dir, file_name + ".json")) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def same_features(fingerprint, features):
    """Does a unit's fingerprint JSON record <features>? It holds them in
    Rust's debug form (["default", "x"]); a form this cannot read counts as
    the same."""
    try:
        return sorted(json.loads(fingerprint["features"])) == sorted(features)
    except (KeyError, ValueError, TypeError):
        return True


def units_by_name(message, path, fingerprints, used):
    """The fingerprint folders of the units a report of <path> can be, for an
    output whose name carries no hash: documentation (<target>/doc/<crate>/...)
    and a cdylib or staticlib (target-macos/debug/libbae_bridge.a). Those are
    the units of the package with this target, mode and features whose
    dependencies' fingerprints are all among <used>, the fingerprints of the
    units this build reported before it (cargo reports a unit after its
    dependencies). When none is, every unit with the target, mode and
    features: their outputs are the same files whichever built them last, so
    keeping each one's fingerprint keeps nothing but those small folders."""
    package = package_name(message["package_id"])
    parent = os.path.dirname(path)
    profiles = [parent] if is_profile_dir(parent) else []
    doc = False
    if not profiles:
        # <target>[/<triple>]/doc/<crate>/index.html, documented from a profile
        # folder beside that doc folder.
        while parent not in ("", os.sep) and not profiles:
            if os.path.basename(parent) == "doc":
                root = os.path.dirname(parent)
                try:
                    names = os.listdir(root)
                except FileNotFoundError:
                    names = []
                profiles = [os.path.join(root, n) for n in names if is_profile_dir(os.path.join(root, n))]
                doc = bool(profiles)
            parent = os.path.dirname(parent)
    name = fingerprint_file(message, doc)
    candidates = []
    for profile in profiles:
        for fingerprint_dir in fingerprints.by_package(profile, package):
            if not os.path.exists(os.path.join(fingerprint_dir, name)):
                continue
            fingerprint = recorded(fingerprint_dir, name)
            if same_features(fingerprint, message.get("features", [])):
                candidates.append((fingerprint_dir, fingerprint))
    current = [
        fingerprint_dir for fingerprint_dir, fingerprint in candidates
        if all(isinstance(dep, list) and dep[-1] in used for dep in fingerprint.get("deps", []))
    ]
    return current or [fingerprint_dir for fingerprint_dir, _ in candidates]


def units_of(message, fingerprints, used):
    """The fingerprint folders of the units a cargo message reports, and the
    paths it reports that name none."""
    reason = message.get("reason")
    if reason == "build-script-executed":
        paths = [message["out_dir"]]
    elif reason == "compiler-artifact":
        paths = list(message.get("filenames") or [])
        if message.get("executable"):
            paths.append(message["executable"])
    else:
        return [], []
    found, missing, unnamed = set(), [], []
    for path in paths:
        unit = hashed_unit(path)
        if unit is None:
            unnamed.append(path)
            continue
        fingerprint_dir = fingerprints.get(*unit)
        if fingerprint_dir:
            found.add(fingerprint_dir)
        else:
            missing.append(path)
    if not found and reason == "compiler-artifact":
        for path in unnamed:
            for unit in copied_from(path):
                fingerprint_dir = fingerprints.get(*unit)
                if fingerprint_dir:
                    found.add(fingerprint_dir)
        if not found:
            for path in unnamed:
                found.update(units_by_name(message, path, fingerprints, used))
        if not found:
            missing.extend(unnamed)
    return sorted(found), missing


def record():
    """Reads cargo's output on stdin. Each of cargo's own messages marks the
    units it reports as used now; every other line (a `cargo run` program's
    output, a test run's) passes through to stdout unchanged."""
    fingerprints = Fingerprints()
    used = set()
    failures = []
    out = sys.stdout.buffer
    out_open = True
    for line in sys.stdin.buffer:
        message = None
        if line.startswith(b'{"reason":'):
            try:
                message = json.loads(line)
            except ValueError:
                message = None
        if message is None:
            if out_open:
                try:
                    out.write(line)
                    out.flush()
                except BrokenPipeError:
                    out_open = False
            continue
        found, missing = units_of(message, fingerprints, used)
        for fingerprint_dir in found:
            try:
                os.utime(fingerprint_dir)
            except FileNotFoundError:
                missing.append(fingerprint_dir)
                continue
            used |= fingerprint_values(fingerprint_dir)
        for path in missing:
            failures.append(f"cargo reported {path} ({message['reason']} of "
                            f"{message.get('package_id', '?')}), and no build copy of it "
                            "has a fingerprint folder to record its use in")
    if not out_open:
        # Python flushes stdout again at exit; point it somewhere open.
        os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
    for failure in failures:
        note(failure)
    return 1 if failures else 0


# ── Pruning ─────────────────────────────────────────────────────────────────


def disk_size(path, seen):
    """Bytes <path> takes on disk, a file or a whole folder, counting each
    hard-linked file once across calls sharing <seen>."""
    total = 0
    try:
        stats = [os.lstat(path)]
    except FileNotFoundError:
        return 0
    if os.path.isdir(path) and not os.path.islink(path):
        for root, dirs, files in os.walk(path):
            for name in dirs + files:
                try:
                    stats.append(os.lstat(os.path.join(root, name)))
                except FileNotFoundError:
                    pass
    for st in stats:
        if st.st_nlink > 1:
            key = (st.st_dev, st.st_ino)
            if key in seen:
                continue
            seen.add(key)
        total += st.st_blocks * 512
    return total


def newest_mtime(path, depth=1):
    """The newest modification time of <path> and what it holds, <depth>
    levels down."""
    try:
        newest = os.lstat(path).st_mtime
    except FileNotFoundError:
        return 0
    if depth and os.path.isdir(path) and not os.path.islink(path):
        try:
            for entry in os.scandir(path):
                newest = max(newest, newest_mtime(entry.path, depth - 1))
        except FileNotFoundError:
            pass
    return newest


class Unit:
    def __init__(self, unit_hash):
        self.hash = unit_hash
        self.name = None
        self.fingerprint = None
        self.build = None
        self.outputs = []

    def paths(self):
        # The fingerprint goes first: without it cargo rebuilds the unit
        # before anything reads its outputs, so a deletion cut off halfway
        # leaves only files the unit's next build overwrites. Until then those
        # files count as used when they were written (last_use).
        return ([self.fingerprint] if self.fingerprint else []) + self.outputs + (
            [self.build] if self.build else [])

    def last_use(self):
        if self.fingerprint:
            return newest_mtime(self.fingerprint)
        return max(newest_mtime(path) for path in self.paths())


def units_in(profile):
    """Every unit in <profile>, by hash."""
    units = {}

    def unit(unit_hash):
        if unit_hash not in units:
            units[unit_hash] = Unit(unit_hash)
        return units[unit_hash]

    for name in os.listdir(os.path.join(profile, ".fingerprint")):
        match = UNIT_DIR.match(name)
        if match:
            found = unit(match["hash"])
            found.fingerprint = os.path.join(profile, ".fingerprint", name)
            found.name = match["package"]
    build = os.path.join(profile, "build")
    if os.path.isdir(build):
        for name in os.listdir(build):
            match = UNIT_DIR.match(name)
            if match:
                found = unit(match["hash"])
                found.build = os.path.join(build, name)
                found.name = found.name or match["package"]
    for folder in OUTPUT_DIRS:
        outputs = os.path.join(profile, folder)
        if not os.path.isdir(outputs):
            continue
        for name in os.listdir(outputs):
            match = HASHED_FILE.match(name)
            if match:
                found = unit(match["hash"])
                found.outputs.append(os.path.join(outputs, name))
                found.name = found.name or match["crate"]
    return units


def incremental_dirs(profile, units):
    """Each folder in <profile>/incremental with the hashes of the units whose
    deps files are hard links to its files (none: no unit's)."""
    incremental = os.path.join(profile, "incremental")
    if not os.path.isdir(incremental):
        return []
    crate_dirs = [os.path.join(incremental, name) for name in os.listdir(incremental)]
    crate_dirs = [path for path in crate_dirs if os.path.isdir(path)]
    if not crate_dirs:
        return []
    owner = {}
    for found in units.values():
        for path in found.outputs:
            try:
                st = os.lstat(path)
            except FileNotFoundError:
                continue
            if st.st_nlink > 1:
                owner[(st.st_dev, st.st_ino)] = found.hash
    result = []
    for crate_dir in crate_dirs:
        owners = set()
        for root, _dirs, files in os.walk(crate_dir):
            for name in files:
                try:
                    st = os.lstat(os.path.join(root, name))
                except FileNotFoundError:
                    continue
                if st.st_nlink > 1 and (st.st_dev, st.st_ino) in owner:
                    owners.add(owner[(st.st_dev, st.st_ino)])
        result.append((crate_dir, owners))
    return result


class OpenFiles:
    """The files and folders under <roots> that a process other than this one
    holds open (a file, a working directory, a running executable), by inode:
    lsof names a hard-linked file by whichever of its names it finds, and
    rustc hard-links a unit's deps files into its incremental folder. Listed
    once, when first asked, which is after the folder's locks are held."""

    def __init__(self, roots):
        self.roots = tuple(root + os.sep for root in roots)
        self.inodes = None

    def get(self):
        if self.inodes is None:
            me = str(os.getpid())
            self.inodes, pid, inode = set(), None, None
            listing = subprocess.run(["lsof", "-n", "-Fpin"], capture_output=True, text=True).stdout
            for line in listing.splitlines():
                if line.startswith("p"):
                    pid = line[1:]
                elif line.startswith("f"):
                    inode = None
                elif line.startswith("i"):
                    inode = line[1:]
                elif line.startswith("n") and pid != me and inode and line[1:].startswith(self.roots):
                    self.inodes.add(int(inode))
        return self.inodes


def holds_any(paths, inodes):
    """Does a process hold any of <paths>, or a file inside one, open?"""
    if not inodes:
        return False
    for path in paths:
        try:
            if os.lstat(path).st_ino in inodes:
                return True
        except FileNotFoundError:
            continue
        if os.path.isdir(path) and not os.path.islink(path):
            for root, dirs, files in os.walk(path):
                for name in dirs + files:
                    try:
                        if os.lstat(os.path.join(root, name)).st_ino in inodes:
                            return True
                    except FileNotFoundError:
                        pass
    return False


def remove(path):
    try:
        if os.path.isdir(path) and not os.path.islink(path):
            shutil.rmtree(path)
        else:
            os.remove(path)
    except FileNotFoundError:
        pass


def lock_profile(profile):
    """Takes cargo's lock on <profile> without waiting, as cargo holds it for a
    whole build: returns its descriptor, or None while a build holds it."""
    fd = os.open(os.path.join(profile, ".cargo-lock"), os.O_RDWR | os.O_CREAT, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as error:
        os.close(fd)
        if error.errno in (errno.EWOULDBLOCK, errno.EAGAIN):
            return None
        raise
    return fd


def prune_profile(profile, cutoff, days, dry, open_files):
    """Deletes <profile>'s units no build used since <cutoff>, and the
    incremental folders only they used, but none a process holds a file of.
    Returns the bytes freed (or that would be)."""
    units = units_in(profile)
    stale = {h: u for h, u in units.items() if u.last_use() < cutoff}
    increments = [
        (crate_dir, owners) for crate_dir, owners in incremental_dirs(profile, units)
        if (owners and owners <= stale.keys())
        or (not owners and newest_mtime(crate_dir, 2) < cutoff)
    ]
    if not stale and not increments:
        return 0
    held = open_files.get()
    kept = [h for h, found in stale.items() if holds_any(found.paths(), held)]
    for unit_hash in kept:
        del stale[unit_hash]
    increments = [
        crate_dir for crate_dir, owners in increments
        if owners <= stale.keys() and not holds_any([crate_dir], held)
    ]
    seen, freed, by_name = set(), 0, {}
    for found in stale.values():
        size = sum(disk_size(path, seen) for path in found.paths())
        freed += size
        count, total = by_name.get(found.name, (0, 0))
        by_name[found.name] = (count + 1, total + size)
        if not dry:
            for path in found.paths():
                remove(path)
    for crate_dir in increments:
        freed += disk_size(crate_dir, seen)
        if not dry:
            remove(crate_dir)
    if stale or increments:
        what = [counted(len(stale), "build copy", "build copies")] if stale else []
        if increments:
            what.append(counted(len(increments), "incremental folder", "incremental folders"))
        most = sorted(by_name.items(), key=lambda item: -item[1][1])[:3]
        detail = "; most: " + ", ".join(
            f"{name} x{count} {human(size)}" for name, (count, size) in most) if most else ""
        note(f"{'would delete' if dry else 'deleted'} {' and '.join(what)} no build used in "
             f"{days} days from {profile} ({human(freed)}{detail})")
    if kept:
        note(f"kept {counted(len(kept), 'unused build copy', 'unused build copies')} in {profile}: a process holds their files")
    return freed


def prune(days, dry, folders):
    """Prunes each build folder's profile folders, each while holding its
    cargo lock (a dry run takes none and deletes nothing). Prints
    "<KiB freed>\\t<folder>" per folder on stdout, notes on stderr."""
    cutoff = time.time() - days * 86400
    for folder in folders:
        root = os.path.realpath(folder)
        profiles, locks = profile_dirs(root), []
        try:
            if not dry:
                free = []
                for profile in profiles:
                    fd = lock_profile(profile)
                    if fd is None:
                        # Counted without the lock, only to say what waits.
                        waiting = sum(1 for u in units_in(profile).values() if u.last_use() < cutoff)
                        if waiting:
                            note(f"kept {counted(waiting, 'unused build copy', 'unused build copies')} in {profile}: "
                                 "a build holds its lock")
                    else:
                        locks.append(fd)
                        free.append(profile)
                profiles = free
            open_files = OpenFiles([root])
            freed = sum(prune_profile(profile, cutoff, days, dry, open_files) for profile in profiles)
        finally:
            for fd in locks:
                os.close(fd)
        print(f"{freed // 1024}\t{folder}")
    return 0


def main(argv):
    if argv[:1] == ["record"] and len(argv) == 1:
        return record()
    if argv[:1] == ["prune"] and len(argv) >= 3:
        return prune(int(argv[1]), argv[2] == "1", argv[3:])
    print("usage: cargo_units.py record | prune <days> <dry 0|1> <folder>...", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

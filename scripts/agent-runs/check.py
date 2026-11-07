#!/usr/bin/env python3
"""Flag running cargo builds and tests that quick mode doesn't ask for.

Quick mode wants each agent to build with the one feature set its repo's hook
builds (another set is another copy of every crate) and to run only the test
that proves its change. This lists every running `cargo test|check|clippy`
with its worktree and says what about it breaks that:

- a feature set other than the repo's hook set;
- a test run with no name filter (a whole suite);
- a test run over more than one crate.

    check.py
"""

import re
import subprocess

# What each repo's hook builds with. coven's builds with --all-features. bae's
# adds, per covered crate, the test features it declares (scripts/hooks/lib/
# file-list.sh crate_check_plan); bae-bridge's tests use the bridge set.
BAE_TEST_FEATURES = {"test-utils", "test-instrumentation"}
BAE_BRIDGE_SET = {"oauth-providers", "cloudkit", "cast", "test-keyring", "dev-reset"}


def hook_builds(repo, found):
    if repo == "coven":
        return found == {"--all-features"}
    # A feature named "crate/feature" belongs to that crate, and each crate's
    # set is judged on its own: one clippy over several crates names each
    # crate's hook set, bae-bridge's bridge set among them.
    per_crate = {}
    for f in found:
        if f == "--all-features":
            return False
        for name in f.removeprefix("--features ").replace(",", " ").split():
            crate, _, feature = name.rpartition("/")
            per_crate.setdefault(crate, set()).add(feature)
    return all(
        names <= BAE_TEST_FEATURES or names == BAE_BRIDGE_SET for names in per_crate.values()
    )


def cwd_of(pid):
    out = subprocess.run(
        ["lsof", "-a", "-d", "cwd", "-p", pid, "-Fn"], capture_output=True, text=True
    ).stdout
    for line in out.splitlines():
        if line.startswith("n"):
            return line[1:]
    return "?"


def run_by_hook(pid):
    """Whether a pre-commit hook or a landing started this run: their builds
    are the gates quick mode keeps."""
    while pid and pid != "1":
        out = subprocess.run(
            ["ps", "-o", "ppid=,command=", "-p", pid], capture_output=True, text=True
        ).stdout.strip()
        if not out:
            return False
        ppid, _, cmd = out.partition(" ")
        if any(m in cmd for m in ("hooks/pre-commit", "check-fast.sh", "land/land.sh", ".land-compile")):
            return True
        pid = ppid.strip()
    return False


def features(args):
    found = set()
    for i, a in enumerate(args):
        if a == "--all-features":
            found.add("--all-features")
        elif a == "--features" and i + 1 < len(args):
            found.add(f"--features {args[i + 1]}")
        elif a.startswith("--features="):
            found.add(f"--features {a.split('=', 1)[1]}")
    return found


def main():
    ps = subprocess.run(
        ["ps", "-axo", "pid=,etime=,command="], capture_output=True, text=True
    ).stdout
    flagged = 0
    for line in ps.splitlines():
        m = re.match(r"\s*(\d+)\s+(\S+)\s+(.*)", line)
        if not m:
            continue
        pid, etime, cmd = m.groups()
        # The wrapper's own line; the real cargo below it repeats the command.
        if "/.local/bin/builds cargo" not in cmd:
            continue
        args = cmd.split()
        kind = next((a for a in args if a in ("test", "check", "clippy")), None)
        if not kind:
            continue
        if run_by_hook(pid):
            continue
        cwd = cwd_of(pid)
        repo = "coven" if "/coven" in cwd else "bae" if "/bae" in cwd else None
        problems = []
        if repo and not hook_builds(repo, features(args)):
            problems.append(f"feature set {sorted(features(args)) or ['default']}, not the hook's")
        # --no-run only builds the test binary, so it runs no suite.
        if kind == "test" and "--no-run" not in args:
            rest = args[args.index("test") + 1 :]
            names = [a for a in rest if not a.startswith("-")]
            # Values of flags that take one aren't test names.
            for flag in ("-p", "--package", "--features", "--test", "--bin", "--target"):
                for i, a in enumerate(rest):
                    if a == flag and i + 1 < len(rest) and rest[i + 1] in names:
                        names.remove(rest[i + 1])
            if not names:
                problems.append("test run with no name filter")
            if sum(1 for a in rest if a in ("-p", "--package")) > 1 or "--workspace" in rest:
                problems.append("test run over more than one crate")
        if problems:
            flagged += 1
            short = cwd.replace("/Users/dima/dev/", "")
            print(f"{short}  pid {pid}, {etime}: {' '.join(args[2:])[:160]}")
            for p in problems:
                print(f"    {p}")
    if not flagged:
        print("agent runs: nothing outside quick mode")


if __name__ == "__main__":
    main()

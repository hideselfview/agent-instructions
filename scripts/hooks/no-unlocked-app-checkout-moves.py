#!/usr/bin/env python3
"""PreToolUse hook on Bash: refuse a command that moves a repo's app checkout
outside its app lock.

The app checkout (`builds app-checkout`, one per repo) is where agents build
Gradle, Xcode and bridge output, one at a time, under the repo's app lock. A
`git checkout` there that does not hold the lock switches the tree under
another agent's build mid-run. The lock is taken by running the command under
`lockf -k <app.lock>`; a git command that changes HEAD or the tree in the app
checkout (cd into it, `git -C <it>`, or a working directory inside it) is
refused unless it is inside that `lockf`. Read-only git is not affected.

The checkouts and lock paths come from `builds status --json`, which records
them, so any repo with an app checkout recorded is covered. When `builds`
fails the command is allowed, with a warning on stderr (exit 1, which Claude
Code shows without blocking).

Known false refusal, kept because refusing too much is the safe side: a `cd`
inside `( ... )` is read as lasting to the end of the command, so
`(cd <app checkout> && git status); git checkout x` run elsewhere is refused.
"""

import json
import os
import subprocess
import sys
from pathlib import Path

# The hooks run under `python3 -I`, which leaves the script's own folder off
# the import path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import shell_words  # noqa: E402

REASON = (
    "This changes the app checkout's HEAD or tree without holding its app lock: another agent's build "
    "there gets its tree switched mid-run. Run it under `lockf -k <repo's app.lock> sh -c '...'` "
    "(see the repo's CLAUDE.md), or do it in your own worktree."
)

# git subcommands that change HEAD, the index or the working tree.
MUTATING = {
    "checkout", "switch", "reset", "clean", "rebase", "cherry-pick", "merge", "stash",
    "restore", "pull", "revert", "am", "commit", "add", "rm", "mv", "apply", "update-ref",
    "symbolic-ref", "worktree", "update-index", "read-tree",
}
# Global options that take a value, which are not the subcommand.
GIT_VALUE_OPTIONS = {"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--super-prefix"}
SHELLS = {"sh", "bash", "zsh", "dash"}
CD = {"cd", "pushd"}


def app_checkouts() -> list[tuple[str, str]]:
    """(app checkout path, app lock path) for each repo with an app checkout recorded.
    Raises OSError, ValueError, KeyError or SubprocessError when `builds` cannot say."""
    result = subprocess.run(["builds", "status", "--json", "--no-folders"], capture_output=True, text=True,
                            timeout=8, check=True)
    repos = json.loads(result.stdout)["repos"]
    return [
        (c["path"], repo["app_lock"])
        for repo in repos
        for c in repo.get("checkouts", [])
        if c.get("app_checkout")
    ]


def expand(path: str) -> str:
    home = str(Path.home())
    path = path.replace("${HOME}", home).replace("$HOME", home)
    return os.path.expanduser(path)


def resolve(path: str, cwd: str) -> str:
    full = os.path.normpath(os.path.join(cwd, expand(path)))
    return os.path.realpath(full)


def within(directory: str, root: str) -> bool:
    root = os.path.realpath(root)
    return directory == root or directory.startswith(root + os.sep)


def git_change(words: list[str], cwd: str):
    """For a `git ...` command that changes HEAD or the tree: the directory it runs in; else None."""
    if words[:1] != ["git"]:
        return None
    here, i = cwd, 1
    while i < len(words) and words[i].startswith("-"):
        if words[i] == "-C" and i + 1 < len(words):
            here = resolve(words[i + 1], here)
        i += 2 if words[i] in GIT_VALUE_OPTIONS else 1
    if i >= len(words) or words[i] not in MUTATING:
        return None
    sub, rest = words[i], words[i + 1:]
    if sub in {"stash", "worktree"} and rest[:1] and rest[0] in {"list", "show"}:
        return None
    if sub == "symbolic-ref" and len([w for w in rest if not w.startswith("-")]) < 2:
        return None  # reading the ref
    if sub in {"commit", "add", "rm", "mv", "apply"} and any(w in {"-n", "--dry-run", "--check"} for w in rest):
        return None
    if sub == "clean" and any(w in {"-n", "--dry-run"} or (w.startswith("-") and not w.startswith("--") and "n" in w)
                              for w in rest):
        return None
    return here


def inner_script(words: list[str]):
    """The script of `sh -c '<script>'`, also behind a wrapper such as `lockf -k <lock>`; else None."""
    for i, word in enumerate(words[:-2]):
        if word in SHELLS and words[i + 1] == "-c":
            return words[i + 2]
    return None


def blocked(command: str, cwd: str, checkouts: list[tuple[str, str]]) -> bool:
    cwd = resolve(".", cwd)
    for words, _ in shell_words.commands(command):
        if not words:
            continue
        if words[0] in CD and len(words) > 1:
            cwd = resolve(words[1], cwd)
            continue
        if words[0] == "lockf" and any(os.path.realpath(expand(w)) in {os.path.realpath(lock) for _, lock in checkouts}
                                       for w in words[1:]):
            continue  # everything this lockf runs holds an app lock
        script = inner_script(words)
        if script is not None:
            if blocked(script, cwd, checkouts):
                return True
            continue
        where = git_change(words, cwd)
        if where is not None and any(within(where, path) for path, _ in checkouts):
            return True
    return False


def main():
    payload = json.load(sys.stdin)
    command = payload.get("tool_input", {}).get("command", "")
    cwd = payload.get("cwd") or os.getcwd()
    if "git" not in command:
        return
    try:
        checkouts = app_checkouts()
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"no-unlocked-app-checkout-moves: allowed unchecked, `builds status --json` failed: {error!r}",
              file=sys.stderr)
        sys.exit(1)
    if blocked(command, cwd, checkouts):
        print(json.dumps({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": REASON,
            }
        }))


if __name__ == "__main__":
    main()

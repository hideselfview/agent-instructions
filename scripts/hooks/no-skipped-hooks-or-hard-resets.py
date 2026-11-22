#!/usr/bin/env python3
"""PreToolUse hook on Bash: refuse a git command that skips the commit hook or
throws away work.

`git commit --no-verify` (or `-n`) and `git push --no-verify` skip the hook that
formats and checks what changed, and `git reset --hard` discards commits and
uncommitted files with no way back. The person allows the first only on request
(~/.claude/CLAUDE.md, "Skip hooks only when the user asks"), and the coordinator
briefs name both as hard limits (`builds help workflow`, DISPATCH). Agents ran
both anyway, so the hook refuses them. A command the person runs with the `!`
prefix does not pass through this hook.
"""

import json
import re
import sys
from pathlib import Path

# The hooks run under `python3 -I`, which leaves the script's own folder off
# the import path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import shell_words  # noqa: E402

REASON = (
    "no-skipped-hooks-or-hard-resets refuses `git commit --no-verify` (and `-n`), any git command "
    "with `--no-verify`, and `git reset --hard`. The commit hook formats and checks what changed: "
    "commit through it and fix what it reports. To drop a commit, use `git rebase` or `git revert`; "
    "to move a branch, use `git checkout -B <branch> <commit>`; when none of these fits, ask the "
    "coordinator. The person may skip the hook or reset hard with a `!` command in the session."
)

ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
# Programs that run the command after them, with their options that take a
# value and the number of arguments of their own.
WRAPPERS = {
    "nohup": (set(), 0), "exec": ({"-a"}, 0), "command": (set(), 0), "time": (set(), 0),
    "nice": ({"-n"}, 0), "env": ({"-u", "-P", "-S"}, 0),
    "timeout": ({"-s", "-k"}, 1), "gtimeout": ({"-s", "-k"}, 1),
}
# Git's own options that take a value in the next word.
GIT_VALUE_OPTIONS = {"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--super-prefix"}
# `git commit` short options that take a value, in the same word or the next:
# the rest of the word after one is that value, not more flags.
COMMIT_VALUE_FLAGS = set("mFcCtSu")


def program(words: list[str]) -> list[str]:
    """The words of the program a command runs, from its name on: without the
    variable assignments before it, the wrappers that run it and its
    redirections."""
    i = 0
    while i < len(words) and ASSIGNMENT.match(words[i]):
        i += 1
    while i < len(words) and words[i] in WRAPPERS:
        value_options, arguments = WRAPPERS[words[i]]
        i += 1
        while i < len(words) and (words[i].startswith("-") or ASSIGNMENT.match(words[i])):
            i += 2 if words[i] in value_options else 1
        i += arguments
    return shell_words.without_redirections(words[i:])


def git_subcommand(words: list[str]):
    """(subcommand, its arguments) of a git command, or None when the words
    are no git command or name no subcommand."""
    if not words or Path(words[0]).name != "git":
        return None
    i = 1
    while i < len(words) and words[i].startswith("-"):
        i += 2 if words[i] in GIT_VALUE_OPTIONS else 1
    if i >= len(words):
        return None
    return words[i], words[i + 1:]


def options(arguments: list[str]) -> list[str]:
    """The arguments before a `--`: after it come paths."""
    return arguments[: arguments.index("--")] if "--" in arguments else arguments


def commit_short_flags(arguments: list[str]) -> set[str]:
    """The short flags of `git commit` arguments, such as `n` for `-anm`,
    leaving out the value that follows a flag taking one (`-mnote`)."""
    flags: set[str] = set()
    for word in options(arguments):
        if not re.fullmatch(r"-[A-Za-z]+", word):
            continue
        for letter in word[1:]:
            flags.add(letter)
            if letter in COMMIT_VALUE_FLAGS:
                break
    return flags


def skips_hook(subcommand: str, arguments: list[str]) -> bool:
    return "--no-verify" in options(arguments) or (
        subcommand == "commit" and "n" in commit_short_flags(arguments)
    )


def hard_resets(subcommand: str, arguments: list[str]) -> bool:
    return subcommand == "reset" and "--hard" in options(arguments)


def blocked(command: str) -> bool:
    for words, _ in shell_words.commands(command):
        found = git_subcommand(program(words))
        if found and (skips_hook(*found) or hard_resets(*found)):
            return True
    return False


def main():
    payload = json.load(sys.stdin)
    command = payload.get("tool_input", {}).get("command", "")
    if blocked(command):
        print(json.dumps({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": REASON,
            }
        }))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""PreToolUse hook on Bash: allow `builds ack` only as a bare command.

`builds ack` prints the text every agent reads before it works, or the
sections of it that changed, in parts, and each part's last line gives the
next command. An agent that pipes the output through `head` or `tail -1`,
saves it to a file it never reads or runs it from a script sees only that next
command, so the token chain passes without the reading it stands for. The hook
allows exactly `builds ack`, `builds ack part N TOKEN` and `builds ack VERSION
TOKEN`, with `--agent ID`, `--project P` and `--role ROLE` after them, and with
one `cd PATH && ` before them (PATH is one plain word) for an agent whose Bash
tool starts in another folder. It refuses every other command that runs
`builds ack`, except `--compacted` and `--codex-compacted`, which read a hook's
input and print no text.
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
    "ack-part-bare refuses a command that runs `builds ack` in any form but the bare one, so "
    "that the whole text reaches your context: no pipe, redirect, `;`, `&&`, `||`, "
    "subshell, `$(...)`, backticks, variable prefix, loop or script around it. Run exactly "
    "`builds ack`, `builds ack part N TOKEN` or `builds ack VERSION TOKEN`, optionally followed by "
    "`--agent ID`, `--project P` and `--role ROLE`, as the previous output's last line gives it. "
    "The one addition allowed is a single leading "
    "`cd PATH && `, where PATH is one plain word of letters, digits, `/`, `.`, `_`, `-` and `~`."
)

NAME = r"[A-Za-z0-9_.-]+"
ROLES = rf"(?: --role {NAME})*"
OPTIONS = rf"(?: --agent {NAME})?(?: --project {NAME})?{ROLES}|(?: --project {NAME})?(?: --agent {NAME})?{ROLES}"
CD = r"(?:cd [A-Za-z0-9/._~-]+ && )?"
WORD = r"[A-Za-z0-9]+"
BARE = re.compile(rf"{CD}builds ack(?: part \d+ {WORD}| {WORD} {WORD})?(?:{OPTIONS})")
# Programs that run a command given as one string.
STRING_RUNNERS = {"bash", "sh", "zsh", "dash", "ksh", "eval", "xargs", "source", "."}
IN_STRING = re.compile(r"\bbuilds\b.*\back\b", re.S)
# The switches of the commands a Claude Code hook runs: they read the hook's
# input and print no text to read.
HOOK_SWITCHES = {"--compacted", "--codex-compacted"}


def subcommand(words: list[str], at: int) -> str | None:
    """The builds subcommand of the command whose `builds` is words[at]: the
    first word after the flags, each taken with its value."""
    i = at + 1
    while i < len(words) and words[i].startswith("--"):
        i += 2
    return words[i] if i < len(words) else None


def runs_ack(command: str) -> bool:
    """Some command in `command` runs `builds ack` for its text, or a shell
    (`bash -c '...'`) or `eval` gets it as a string. Read as shell words, so
    the same words in a quoted commit message or grep pattern are text."""
    for words, _ in shell_words.commands(command):
        words = [re.split(r"[`=]", w)[-1] for w in words]
        for i, word in enumerate(words):
            if (
                Path(word).name == "builds"
                and subcommand(words, i) == "ack"
                and not HOOK_SWITCHES.intersection(words)
            ):
                return True
        if any(Path(w).name in STRING_RUNNERS for w in words[:1]) and any(
            IN_STRING.search(w) and not HOOK_SWITCHES.intersection(w.split())
            for w in words[1:]
        ):
            return True
    return False


def blocked(command: str) -> bool:
    return runs_ack(command) and not BARE.fullmatch(command.strip())


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

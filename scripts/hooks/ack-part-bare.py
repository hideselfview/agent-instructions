#!/usr/bin/env python3
"""PreToolUse hook on Bash: allow `builds ack part` only as a bare command.

`builds ack part N TOKEN` prints one part of the text every agent reads before
it works, and its last line gives the next command. An agent that pipes the
output through `tail -1`, saves it to a file it never reads or runs it from a
script sees only that next command, so the token chain passes without the
reading it stands for. The hook allows exactly `builds ack part N TOKEN`, with
`--agent ID` and `--project P` after it, and refuses every other command that
runs `builds ack part`.
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
    "ack-part-bare refuses a command that runs `builds ack part` in any form but the bare one, so "
    "that the whole text of the part reaches your context: no pipe, redirect, `;`, `&&`, `||`, "
    "subshell, `$(...)`, backticks, variable prefix, loop or script around it. Run exactly "
    "`builds ack part N TOKEN`, optionally followed by `--agent ID` and `--project P`, as the "
    "previous part's last line gives it."
)

NAME = r"[A-Za-z0-9_.-]+"
OPTIONS = rf"(?: --agent {NAME})?(?: --project {NAME})?|(?: --project {NAME})?(?: --agent {NAME})?"
BARE = re.compile(rf"builds ack part \d+ [A-Za-z0-9]+(?:{OPTIONS})")
# Programs that run a command given as one string.
STRING_RUNNERS = {"bash", "sh", "zsh", "dash", "ksh", "eval", "xargs", "source", "."}
IN_STRING = re.compile(r"\bbuilds\b.*\back\s+part\b", re.S)


def runs_ack_part(command: str) -> bool:
    """Some command in `command` runs `builds ... ack part`, or a shell
    (`bash -c '...'`) or `eval` gets it as a string. Read as shell words, so
    the same words in a quoted commit message or grep pattern are text."""
    for words, _ in shell_words.commands(command):
        words = [re.split(r"[`=]", w)[-1] for w in words]
        for i, word in enumerate(words):
            if Path(word).name == "builds" and any(
                words[j:j + 2] == ["ack", "part"] for j in range(i + 1, len(words))
            ):
                return True
        if any(Path(w).name in STRING_RUNNERS for w in words[:1]) and any(
            IN_STRING.search(w) for w in words[1:]
        ):
            return True
    return False


def blocked(command: str) -> bool:
    return runs_ack_part(command) and not BARE.fullmatch(command.strip())


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

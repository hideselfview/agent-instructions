#!/usr/bin/env python3
"""PreToolUse hook on Bash: refuse a loop that waits on pgrep or sleeps between checks.

`until ! pgrep -f "<pattern>"; do sleep 5; done` never ends: the loop's own
command line holds the pattern, so pgrep always finds it. With other agents
building, a pattern like "cargo" always matches someone else's process too.
Such loops ran for 15 hours before anyone killed them. Work that takes a while
runs in the background, and the harness reports when it exits.
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
    "A while/until loop that polls (on pgrep, or sleeping between checks), or "
    "a `tail -f | grep` wait, can run for hours after its condition came true "
    "or when it never does. Run the work in the background (run_in_background) "
    "and wait for its completion notification."
)

LOOP = re.compile(r"\b(while|until)\b(?P<cond>.*?)\bdo\b(?P<body>.*?)\bdone\b", re.S)
FOLLOW = re.compile(r"-[a-zA-Z]*[fF][a-zA-Z]*|--follow\S*")


def follows_into_pipe(command: str) -> bool:
    """`tail -f`/`-F` (or --follow) piped into a reader: tail follows the file
    forever, and only notices its reader has exited on its next write, so
    `tail -F log | grep -m1 done` outlives its match for as long as the log is
    quiet (one ran 2 hours after its line came). Read as shell words, so the
    same words inside a quoted string (a commit message) are text."""
    return any(
        separator == "|" and words[:1] == ["tail"] and any(FOLLOW.fullmatch(w) for w in words[1:])
        for words, separator in shell_words.commands(command)
    )


def blocked(command: str) -> bool:
    # A loop on pgrep, or any loop that sleeps between checks: both poll for a
    # condition the harness already reports, and a condition that never comes
    # true (a pattern matching the loop itself, a log line that is never the
    # last one) keeps the shell running for hours.
    return follows_into_pipe(command) or any(
        "pgrep" in m.group("cond") or re.search(r"\bsleep\b", m.group("body"))
        for m in LOOP.finditer(command)
    )


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

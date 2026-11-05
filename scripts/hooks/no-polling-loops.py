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

REASON = (
    "A while/until loop that polls (on pgrep, or sleeping between checks) can "
    "run for hours when its condition never comes true. Run the work in the "
    "background (run_in_background) and wait for its completion notification."
)

LOOP = re.compile(r"\b(while|until)\b(?P<cond>.*?)\bdo\b(?P<body>.*?)\bdone\b", re.S)


def blocked(command: str) -> bool:
    # A loop on pgrep, or any loop that sleeps between checks: both poll for a
    # condition the harness already reports, and a condition that never comes
    # true (a pattern matching the loop itself, a log line that is never the
    # last one) keeps the shell running for hours.
    return any(
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

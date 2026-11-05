#!/usr/bin/env python3
"""PreToolUse hook on Bash: refuse a loop that waits on pgrep.

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
    "A while/until loop on pgrep never ends: its own command line matches the "
    "pattern, and other agents' builds match it too. Run the work in the "
    "background (run_in_background) and wait for its completion notification."
)

LOOP = re.compile(r"\b(while|until)\b(?P<cond>.*?)\bdo\b", re.S)


def blocked(command: str) -> bool:
    return any("pgrep" in m.group("cond") for m in LOOP.finditer(command))


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

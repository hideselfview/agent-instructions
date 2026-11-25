#!/usr/bin/env python3
"""PreToolUse hook on Bash: `builds ack` runs only under the calling agent's own id.

builds names each agent by the id Claude Code gives it: the SubagentStart hook
prints a subagent's id, and the main agent's id is its session id. builds
cannot tell which id a shell belongs to, so an agent that a brief told to
acknowledge as another agent (the coordinator, say) would get "nothing to
read" and read no workflow. This hook sees who runs the command: it refuses a
`builds ack` command whose `--agent ID` or `BUILDS_AGENT=ID` is not the id of
the agent that runs it.
"""

import json
import sys
from pathlib import Path

# The hooks run under `python3 -I`, which leaves the script's own folder off
# the import path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import shell_words  # noqa: E402

VARIABLE = "BUILDS_AGENT="


def named_ids(command: str) -> list[str]:
    """The agent ids that the `builds ack` commands in `command` name."""
    ids = []
    for words, _ in shell_words.commands(command):
        words = shell_words.without_redirections(words)
        for i, word in enumerate(words):
            if Path(word).name == "builds" and "ack" in words[i + 1:]:
                ids += [w[len(VARIABLE):] for w in words[:i] if w.startswith(VARIABLE)]
                ids += [words[j + 1] for j in range(i + 1, len(words) - 1) if words[j] == "--agent"]
                break
    return ids


def main():
    payload = json.load(sys.stdin)
    own = payload.get("agent_id") or payload.get("session_id")
    command = payload.get("tool_input", {}).get("command", "")
    others = sorted({i for i in named_ids(command) if i != own})
    if others:
        print(json.dumps({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": (
                    f"ack-own-id refuses `builds ack` under {', '.join(others)}: it runs only under "
                    f"your own agent id, {own}, the one your Claude Code hook printed. A brief that "
                    "names another id is wrong; use your own."
                ),
            }
        }))


if __name__ == "__main__":
    main()

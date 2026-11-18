#!/usr/bin/env python3
"""Stop hook: refuse a turn whose reply promises behavior for later.

A session forgets what it promised, so "from now on" or "next time I'll..."
changes nothing. The agent changes the rule, tool or check that makes the
behavior happen and says what it changed, or says nothing.

The hook reads only the assistant's own text blocks after the last real user
message in the transcript: not tool results, not what the user wrote, not
tool calls or thinking. The transcript is JSONL; each line has `type`
("user" or "assistant"), `isMeta` and `message.content`, which is a string
(a typed prompt) or a list of blocks (`text`, `tool_use`, `tool_result`, ...).
"""

import json
import re
import sys

PHRASES = re.compile(
    r"\b(from now on|going forward|next time|i(?:'|’)ll remember)\b",
    re.IGNORECASE,
)

REASON = (
    "no-future-promises: your reply says {found}, which promises behavior for later. A session "
    "forgets such a promise. Drop it and change the rule, tool or check that makes the behavior "
    "happen (an instruction, a hook, a test), then say what you changed. If nothing can enforce "
    "it, say nothing about later."
)


def is_real_user_message(entry: dict) -> bool:
    """A user entry that holds a prompt, not only tool results, and is not a
    system-injected meta message."""
    if entry.get("type") != "user" or entry.get("isMeta"):
        return False
    content = entry.get("message", {}).get("content")
    if isinstance(content, str):
        return True
    return isinstance(content, list) and any(
        isinstance(block, dict) and block.get("type") != "tool_result" for block in content
    )


def assistant_text(entry: dict) -> list[str]:
    content = entry.get("message", {}).get("content")
    if isinstance(content, str):
        return [content]
    return [
        block["text"]
        for block in content or []
        if isinstance(block, dict) and block.get("type") == "text" and isinstance(block.get("text"), str)
    ]


def turn_text(transcript_path: str) -> str:
    """The assistant's text since the last real user message."""
    text: list[str] = []
    with open(transcript_path, encoding="utf-8") as lines:
        for line in lines:
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(entry, dict) or entry.get("isSidechain"):
                continue
            if is_real_user_message(entry):
                text = []
            elif entry.get("type") == "assistant":
                text.extend(assistant_text(entry))
    return "\n".join(text)


def main():
    payload = json.load(sys.stdin)
    if payload.get("stop_hook_active"):
        return
    try:
        text = turn_text(payload["transcript_path"])
    except (KeyError, OSError):
        return
    found = sorted({m.group(1).lower() for m in PHRASES.finditer(text)})
    if found:
        print(json.dumps({
            "decision": "block",
            "reason": REASON.format(found=" and ".join(f'"{p}"' for p in found)),
        }))


if __name__ == "__main__":
    main()

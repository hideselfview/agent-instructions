#!/usr/bin/env python3
"""Stop hook: refuse a turn whose reply promises behavior for later.

A session forgets what it promised, so "from now on" or "next time I'll..."
changes nothing. The agent changes the rule, tool or check that makes the
behavior happen and says what it changed, or says nothing.

The hook reads only the assistant's own text blocks of the current turn,
outside quotation marks, backticks, code fences and blockquote lines: not tool
results, not what the user wrote, not tool calls or thinking. The transcript is
JSONL; each line has `type` ("user" or "assistant"), `isMeta`, `promptId` (on
user entries) and `message.content`, which is a string (a typed prompt) or a
list of blocks (`text`, `tool_use`, `tool_result`, ...). A turn is the run of
entries that follow a user entry with a new `promptId`: its tool results and the
hook's own feedback entry share that `promptId`, while a typed prompt, a task
notification and a scheduled prompt (which is `isMeta`) each carry a new one.
"""

import json
import re
import sys

PHRASES = re.compile(
    r"\b(from now on|going forward|next time|i(?:'|’)ll remember)\b",
    re.IGNORECASE,
)

QUOTED = re.compile(
    r"```.*?```|`[^`\n]*`|\"[^\"\n]*\"|“[^”\n]*”|^[ \t]*>.*$",
    re.DOTALL | re.MULTILINE,
)

REASON = (
    "no-future-promises: your reply says {found}, which promises behavior for later. A session "
    "forgets such a promise. Drop it. For a later step of work, add a queue item or make the work "
    "wait on an item or question. For behavior that should always hold, edit an existing rule, "
    "skill or CLAUDE.md. Otherwise say nothing about later."
)


def starts_turn(entry: dict, prompt_id: str | None) -> bool:
    """Whether a user entry opens a new turn. An entry with a promptId opens one
    when the id differs from the current turn's. An entry without one (an older
    transcript) opens one when it holds a prompt, not only tool results, and is
    not a system-injected meta message."""
    if entry.get("type") != "user":
        return False
    if entry.get("promptId"):
        return entry["promptId"] != prompt_id
    if entry.get("isMeta"):
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
    """The assistant's text of the last turn."""
    text: list[str] = []
    prompt_id = None
    with open(transcript_path, encoding="utf-8") as lines:
        for line in lines:
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(entry, dict) or entry.get("isSidechain"):
                continue
            if starts_turn(entry, prompt_id):
                text = []
            if entry.get("type") == "user" and entry.get("promptId"):
                prompt_id = entry["promptId"]
            if entry.get("type") == "assistant":
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
    found = sorted({m.group(1).lower() for m in PHRASES.finditer(QUOTED.sub(" ", text))})
    if found:
        print(json.dumps({
            "decision": "block",
            "reason": REASON.format(found=" and ".join(f'"{p}"' for p in found)),
        }))


if __name__ == "__main__":
    main()

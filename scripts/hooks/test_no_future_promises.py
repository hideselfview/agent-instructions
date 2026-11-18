"""Cases for no-future-promises.py: python3 scripts/hooks/test_no_future_promises.py"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

HOOK = str(Path(__file__).with_name("no-future-promises.py"))


def user(text, **extra):
    return {"type": "user", "message": {"role": "user", "content": text}, **extra}


def assistant(*blocks):
    return {"type": "assistant", "message": {"role": "assistant", "content": list(blocks)}}


def say(text):
    return assistant({"type": "text", "text": text})


def tool_result(text):
    return {"type": "user", "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "t1", "content": text}]}}


CASES = [
    (True, "from now on", [user("hi"), say("Done. From now on I will check first.")]),
    (True, "going forward", [user("hi"), say("Going forward, I run the tests.")]),
    (True, "next time", [user("hi"), say("Next time I will ask.")]),
    (True, "I'll remember", [user("hi"), say("I'll remember that.")]),
    (True, "curly apostrophe", [user("hi"), say("I’ll remember that.")]),
    (True, "phrase in an earlier text block of the same turn", [
        user("hi"), say("From now on I check."),
        assistant({"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}),
        tool_result("ok"), say("Done.")]),
    (True, "user content as a list of text blocks", [
        user([{"type": "text", "text": "hi"}]), say("next time")]),
    (False, "no phrase", [user("hi"), say("Done. I changed the hook.")]),
    (False, "no entries", []),
    (False, "phrase only in a tool result", [
        user("hi"), assistant({"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}),
        tool_result("from now on, going forward, next time, I'll remember"), say("Done.")]),
    (False, "phrase only in the user's message", [
        user("Please say from now on you will check"), say("Done.")]),
    (False, "phrase only in the user's message as text blocks", [
        user([{"type": "text", "text": "going forward do X"}]), say("Done.")]),
    (False, "phrase in an earlier turn", [
        user("one"), say("From now on I check."), user("two"), say("Done.")]),
    (False, "phrase in a meta user message", [
        user("hi"), user("next time", isMeta=True), say("Done.")]),
    (False, "phrase in thinking and tool input", [
        user("hi"), assistant(
            {"type": "thinking", "thinking": "from now on"},
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo next time"}}),
        tool_result("ok"), say("Done.")]),
    (False, "phrase in a sidechain entry", [
        user("hi"), {**say("from now on"), "isSidechain": True}, say("Done.")]),
    (False, "a similar word", [user("hi"), say("The next timeout and the going rate.")]),
    (False, "stop_hook_active", [user("hi"), say("From now on I check.")], {"stop_hook_active": True}),
]

bad = 0
with tempfile.TemporaryDirectory() as tmp:
    for n, (want, name, entries, *extra) in enumerate(CASES):
        path = Path(tmp) / f"{n}.jsonl"
        path.write_text("".join(json.dumps(e) + "\n" for e in entries))
        payload = {"transcript_path": str(path), "hook_event_name": "Stop", **(extra[0] if extra else {})}
        out = subprocess.run([sys.executable, "-I", HOOK], input=json.dumps(payload),
                             capture_output=True, text=True).stdout
        got = json.loads(out)["decision"] == "block" if out else False
        print(("ok  " if got == want else "FAIL"), "block" if got else "allow", name)
        bad += got != want
    # A missing transcript lets the stop through.
    out = subprocess.run([sys.executable, "-I", HOOK],
                         input=json.dumps({"transcript_path": f"{tmp}/none.jsonl"}),
                         capture_output=True, text=True)
    print(("ok  " if not out.stdout and out.returncode == 0 else "FAIL"), "allow missing transcript")
    bad += bool(out.stdout) or out.returncode != 0
sys.exit(bad)

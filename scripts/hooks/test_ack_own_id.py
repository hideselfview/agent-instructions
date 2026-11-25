"""Cases for ack-own-id.py: python3 scripts/hooks/test_ack_own_id.py"""

import json
import subprocess
import sys
from pathlib import Path

HOOK = str(Path(__file__).with_name("ack-own-id.py"))
SESSION = "11111111-aaaa"
SUB = "a4145f1c3850e2ab9"
# (denied, hook input, command)
CASES = [
    # A subagent acknowledges under its own id.
    (False, {"agent_id": SUB}, f"builds ack --agent {SUB} --project builds"),
    (False, {"agent_id": SUB}, f"builds ack part 2 7d7c --agent {SUB}"),
    (False, {"agent_id": SUB}, f"builds ack 71849511 a07b --project p --agent {SUB}"),
    (False, {"agent_id": SUB}, f"BUILDS_AGENT={SUB} builds ack"),
    (False, {"agent_id": SUB}, f"cd /tmp && builds ack part 1 T --agent {SUB}"),
    # A subagent acknowledges under another agent's id, such as its coordinator's.
    (True, {"agent_id": SUB}, f"builds ack --agent {SESSION} --project builds"),
    (True, {"agent_id": SUB}, "builds ack part 1 T --agent f42e06a3-5684-4c19"),
    (True, {"agent_id": SUB}, f"builds ack 71849511 a07b --agent {SESSION}"),
    (True, {"agent_id": SUB}, f"BUILDS_AGENT={SESSION} builds ack"),
    (True, {"agent_id": SUB}, f"builds --agent {SESSION} ack"),
    (True, {"agent_id": SUB}, f"cd /tmp && builds ack --agent {SESSION}"),
    (True, {"agent_id": SUB}, f"builds ack --agent {SUB} --agent {SESSION}"),
    # The main agent's id is its session id.
    (False, {}, f"builds ack --agent {SESSION} --project builds"),
    (True, {}, f"builds ack --agent {SUB}"),
    # Commands that are no acknowledgement are no concern of this hook.
    (False, {"agent_id": SUB}, f"builds item show 1 --agent {SESSION}"),
    (False, {"agent_id": SUB}, f"git commit -m 'builds ack --agent {SESSION}'"),
    (False, {"agent_id": SUB}, f"grep -rn 'builds ack --agent {SESSION}' plans"),
    (False, {"agent_id": SUB}, "builds ack --compacted"),
    (False, {"agent_id": SUB}, "builds ack"),
]
bad = 0
for want, extra, cmd in CASES:
    payload = {"session_id": SESSION, "tool_input": {"command": cmd}, **extra}
    out = subprocess.run([sys.executable, "-I", HOOK], input=json.dumps(payload),
                         capture_output=True, text=True).stdout
    got = "deny" in out
    print(("ok  " if got == want else "FAIL"), "deny " if got else "allow", cmd)
    bad += got != want
sys.exit(bad)

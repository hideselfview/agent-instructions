"""Cases for ack-part-bare.py: python3 scripts/hooks/test_ack_part_bare.py"""

import json
import subprocess
import sys
from pathlib import Path

HOOK = str(Path(__file__).with_name("ack-part-bare.py"))
CASES = [
    (False, 'builds ack part 1 3e193a159ac240e9'),
    (False, 'builds ack part 2 7d7c0ebf9f80a2d1 --agent a4145f1c3850e2ab9'),
    (False, 'builds ack part 3 e7779d9a0fd1b923 --agent a41 --project builds'),
    (False, 'builds ack part 3 e7779d9a0fd1b923 --project builds --agent a41'),
    (False, 'builds ack part 3 e7779d9a0fd1b923 --project builds\n'),
    (False, 'builds ack part 2 7d7c --agent a41 --project builds --role coordinator'),
    (False, 'builds ack part 2 7d7c --agent a41 --role design --role judge'),
    (False, 'builds ack part 2 7d7c --role judge'),
    (True, 'builds ack part 2 7d7c --role judge | tail -1'),
    (True, 'builds ack part 2 7d7c --role $ROLE'),
    (True, 'builds ack part 1 3e193a159ac240e9 | tail -1'),
    (True, 'builds ack part 1 3e193a159ac240e9 | head -5'),
    (True, 'builds ack part 1 3e193a159ac240e9 > /tmp/m1.txt; tail -4 /tmp/m1.txt'),
    (True, 'builds ack part 1 T 2>&1 >/tmp/m'),
    (True, 'builds ack part 1 T >> /tmp/m'),
    (True, 'builds ack part 1 T; echo done'),
    (True, 'builds ack part 1 T && builds ack part 2 U'),
    (True, 'builds ack part 1 T || true'),
    (True, 'builds ack part 1 T &'),
    (True, 'builds ack part 1 T\necho done'),
    (True, 'echo $(builds ack part 1 T)'),
    (True, 'x=`builds ack part 1 T`'),
    (True, '(builds ack part 1 T)'),
    (False, 'cd /tmp && builds ack part 1 T'),
    (False, 'cd ~/dev/agent-instructions-worktrees/side-6 && builds ack part 2 7d7c --agent a1 --project builds'),
    (False, 'cd ../x_y.z-1 && builds ack part 1 T --project builds --agent a1\n'),
    (True, 'cd /tmp && builds ack part 1 T | tail -1'),
    (True, 'cd /tmp && builds ack part 1 T > /tmp/m'),
    (True, 'cd /tmp && builds ack part 1 T && echo done'),
    (True, 'cd $DIR && builds ack part 1 T'),
    (True, 'cd "$(pwd)" && builds ack part 1 T'),
    (True, 'cd /tmp; builds ack part 1 T'),
    (True, 'cd /tmp || builds ack part 1 T'),
    (True, 'cd /tmp && cd /var && builds ack part 1 T'),
    (True, 'cd /tmp /var && builds ack part 1 T'),
    (True, 'cd "/tmp dir" && builds ack part 1 T'),
    (True, 'cd /tmp && BUILDS_AGENT=a1 builds ack part 1 T'),
    (True, 'cd /tmp && (builds ack part 1 T)'),
    (True, 'cd /tmp&&builds ack part 1 T'),
    (True, 'BUILDS_AGENT=a1 builds ack part 1 T'),
    (True, 'env BUILDS_AGENT=a1 builds ack part 1 T'),
    (True, 'for n in 1 2; do builds ack part $n T; done'),
    (True, 'builds ack part $N T'),
    (True, 'builds ack part 1 $(cat tok)'),
    (True, 'builds ack part 1 T --agent $(whoami)'),
    (True, 'builds ack part 1 T --agent a1 --agent a2'),
    (True, 'builds ack part 1 T extra'),
    (True, 'builds ack part 1 T < /dev/null'),
    (True, '~/.local/bin/builds ack part 1 T'),
    (True, 'builds --agent a1 ack part 1 T'),
    (True, 'bash -c "builds ack part 1 T | tail -1"'),
    (True, "sh -c 'builds ack part 1 T > /tmp/m'"),
    (True, 'eval "builds ack part 1 T"'),
    (True, 'bash ack.sh builds ack part 1 T'),
    (False, 'builds ack --agent a1 --project p'),
    (False, 'builds ack 71849511 a07b04eb06a71587 --agent a1 --project p'),
    (False, 'builds ack --compacted'),
    (False, 'builds item show 437 --project builds | tail -3'),
    (False, 'git commit -m "a hook that refuses builds ack part 1 T | tail -1"'),
    (False, 'grep -rn "builds ack part" scripts | head'),
    (False, 'echo "ack part" | tail -1'),
    (False, 'bash -c "echo hi" | tail -1'),
]
bad = 0
for want, cmd in CASES:
    out = subprocess.run([sys.executable, "-I", HOOK], input=json.dumps({"tool_input": {"command": cmd}}),
                         capture_output=True, text=True).stdout
    got = "deny" in out
    print(("ok  " if got == want else "FAIL"), "deny " if got else "allow", cmd.replace("\n", " "))
    bad += got != want
sys.exit(bad)

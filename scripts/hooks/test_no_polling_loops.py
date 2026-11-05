"""Cases for no-polling-loops.py: python3 scripts/hooks/test_no_polling_loops.py"""

import json
import subprocess
import sys
from pathlib import Path

HOOK = str(Path(__file__).with_name("no-polling-loops.py"))
CASES = [
    (True, 'until ! pgrep -f "cargo test"; do sleep 5; done'),
    (True, 'while pgrep x >/dev/null; do :; done'),
    (True, 'until grep -q "test result" out.txt; do sleep 10; done; cat out.txt'),
    (True, 'f=x; until grep -q a $f && [ 1 = 1 ]; do\n  sleep 5\ndone'),
    (True, 'while true; do curl -s localhost:1 && break; sleep 2; done'),
    (False, 'for f in a b; do echo $f; sleep 1; done'),
    (False, 'while read -r line; do echo "$line"; done < file'),
    (False, 'echo "sleep well" && grep -q x y'),
    (False, 'git log --oneline | while read -r sha msg; do echo $sha; done'),
    (True, "tail -n +1 -F out.txt 2>/dev/null | grep -m1 'run2'"),
    (True, 'tail -f log | grep -q done'),
    (True, 'tail --follow=name log | head -1'),
    (False, 'tail -5 log | grep x'),
    (False, 'tail -f log'),
    (False, 'tail -n 20 out.txt || tail -f other'),
    (False, 'git diff --stat | tail -1'),
    (False, 'grep -n foo file | tail -3'),
    (False, 'git commit -m "a tail -F | grep -m1 wait outlived its match"'),
    (True, 'cd x && tail -F log | grep -m1 y'),
    (True, 'tail -n 5 -f log|grep y'),
    (False, 'git commit -q -m "first line\n\ntail -F | grep -m1 wait outlived\nits match" && git push'),
    (True, 'echo x\ntail -F log | grep y'),
]
bad = 0
for want, cmd in CASES:
    out = subprocess.run([sys.executable, "-I", HOOK], input=json.dumps({"tool_input": {"command": cmd}}),
                         capture_output=True, text=True).stdout
    got = "deny" in out
    print(("ok  " if got == want else "FAIL"), "deny " if got else "allow", cmd.replace("\n", " "))
    bad += got != want
sys.exit(bad)

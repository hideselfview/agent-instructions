"""Cases for no-full-suites.py: python3 scripts/hooks/test_no_full_suites.py"""

import json
import subprocess
import sys
from pathlib import Path

HOOK = str(Path(__file__).with_name("no-full-suites.py"))
C = "check" + ".sh"
CASES = [
    (True, f"scripts/{C}"),
    (True, f"./scripts/{C} code"),
    (True, f"bash scripts/{C}"),
    (True, f"env A=1 scripts/{C}"),
    (True, f"cd x && scripts/{C}"),
    (True, f"FOO=1 scripts/{C}"),
    (True, f"lockf -k /tmp/l scripts/{C}"),
    (False, f"git show HEAD:scripts/{C}"),
    (False, f"grep deny scripts/{C}"),
    (False, f"git show abc -- scripts/{C} | head"),
    (False, f"sed -n 1,5p scripts/{C}"),
    (True, "scripts/cargo.sh test -p x"),
    (False, "scripts/cargo.sh test -p x foo"),
    (False, f"python3 - <<'EOF'\nx='- `scripts/{C}` never runs'\nEOF\necho done"),
    (False, f"python3 -c 'print(1)' <<\"EOF\"\n`scripts/{C}`\nEOF"),
    (True, f"bash <<'EOF'\nscripts/{C}\nEOF"),
    (True, f"python3 - <<'EOF'\nx=1\nEOF\nscripts/{C}"),
    (True, f"cat <<EOF\n`scripts/{C}`\nEOF"),
    (True, "python3 - <<'EOF'\nx=1\nEOF\nscripts/cargo.sh test -p x"),
    (False, f"ps -Ao command | grep -E \"nextest|{C}|lake \" | grep -v grep"),
    (False, f"grep -n 'a|scripts/{C}' file"),
    (True, f"echo \"$(scripts/{C})\""),
    (True, f"x=$(scripts/{C}) && echo $x"),
    (True, f"echo `scripts/{C}`"),
    (True, f"(cd x; scripts/{C})"),
    (True, "a=1; scripts/cargo.sh test -p x 2>&1 | tail"),
    (False, "scripts/cargo.sh test -p x foo 2>&1 | tail"),
    (True, "scripts/cargo.sh nextest run -p x > log 2>&1 &"),
    (False, "echo hi # scripts/" + C),
    (False, f"git commit -m \"first\n\nnames scripts/{C} | in\ntext\" && git push"),
    (True, "scripts/cargo.sh test -p bae-records --features test-utils --lib ::"),
    (True, "scripts/cargo.sh test -p x -- ::"),
    (True, "scripts/cargo.sh test -p x _"),
    (False, "scripts/cargo.sh test -p x --lib people::"),
    (False, "scripts/cargo.sh test -p x -- tests::a_b"),
    (True, f"echo x\nscripts/{C}"),
]
bad = 0
for want, cmd in CASES:
    # -I as the hook settings run it, so a missing import shows here.
    out = subprocess.run([sys.executable, "-I", HOOK], input=json.dumps({"tool_input": {"command": cmd}}),
                         capture_output=True, text=True).stdout
    got = "deny" in out
    print(("ok  " if got == want else "FAIL"), "deny" if got else "allow", cmd)
    bad += got != want
sys.exit(bad)

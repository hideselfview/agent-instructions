"""Cases for no-unlocked-app-checkout-moves.py: python3 scripts/hooks/test_no_unlocked_app_checkout_moves.py"""

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

HOOK = Path(__file__).with_name("no-unlocked-app-checkout-moves.py")
spec = importlib.util.spec_from_file_location("hook", HOOK)
hook = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hook)

APP = "/Users/dima/dev/bae-worktrees/side-1"
LOCK = "/Volumes/Builds/bae/app.lock"
CHECKOUTS = [(APP, LOCK)]
ELSEWHERE = "/Users/dima/dev/bae-worktrees/side-2"

RECIPE = f"""lockf -k {LOCK} sh -c '
  cd ~/dev/bae-worktrees/side-1 &&
  {{ test -z "$(git status --porcelain)" || {{ git status --short; exit 1; }}; }} &&
  git checkout -q --detach android-test-timeouts &&
  ./bae-macos/run.sh --no-open'"""

# (deny?, cwd, command)
CASES = [
    # The incident: a checkout in the app checkout, with no lock.
    (True, ELSEWHERE, "cd ~/dev/bae-worktrees/side-1 && git checkout -q --detach origin/main"),
    (True, ELSEWHERE, f"git -C {APP} checkout -q --detach origin/main"),
    (True, APP, "git checkout main"),
    (True, APP + "/bae-android", "git reset --hard origin/main"),
    (True, ELSEWHERE, f"cd {APP}\ngit switch -c x"),
    (True, ELSEWHERE, "cd $HOME/dev/bae-worktrees/side-1; git clean -fd"),
    (True, ELSEWHERE, f"git -C {APP} stash"),
    (True, ELSEWHERE, f"cd {APP} && git rebase origin/main"),
    (True, ELSEWHERE, f"cd {APP} && git cherry-pick abc123"),
    (True, ELSEWHERE, f"cd {APP} && git merge --ff-only x"),
    (True, ELSEWHERE, f"sh -c 'cd {APP} && git checkout x'"),
    (True, ELSEWHERE, f"lockf -k /tmp/other.lock sh -c 'cd {APP} && git checkout x'"),
    (True, ELSEWHERE, f"lockf -k {LOCK} true; cd {APP} && git checkout x"),
    # The recipe from CLAUDE.md, and the same checkout moved under the lock.
    (False, ELSEWHERE, RECIPE),
    (False, ELSEWHERE, f"lockf -k {LOCK} git -C {APP} checkout x"),
    (False, "/Users/dima/dev/bae", RECIPE),
    # Read-only git stays allowed in the app checkout.
    (False, APP, "git status --short"),
    (False, ELSEWHERE, f"git -C {APP} log --oneline -3"),
    (False, ELSEWHERE, f"cd {APP} && git diff --stat && git show HEAD && git rev-parse HEAD"),
    (False, ELSEWHERE, f"cd {APP} && git stash list"),
    (False, ELSEWHERE, f"cd {APP} && git clean -n"),
    (False, ELSEWHERE, f"cd {APP} && git branch --show-current && git merge-base HEAD origin/main"),
    # The same changes elsewhere are the agent's own worktree.
    (False, ELSEWHERE, "git checkout -b x origin/main"),
    (False, ELSEWHERE, "cd ~/dev/bae-worktrees/side-2 && git rebase origin/main"),
    (False, "/Users/dima/dev/bae-worktrees/side-1-notes", "git checkout x"),
    # Words in a quoted string are text.
    (False, ELSEWHERE, f'git commit -m "cd {APP} && git checkout x"'),
]
bad = 0
for want, cwd, cmd in CASES:
    got = hook.blocked(cmd, cwd, CHECKOUTS)
    print(("ok  " if got == want else "FAIL"), "deny " if got else "allow", cmd.replace("\n", " ")[:100])
    bad += got != want

# End to end through the hook's stdin protocol, with the app checkout `builds` records.
live = hook.app_checkouts()
if live:
    path, lock = live[0]
    for want, cmd in [
        (True, f"cd {path} && git checkout -q --detach origin/main"),
        (False, f"lockf -k {lock} sh -c 'cd {path} && git checkout -q --detach x'"),
        (False, f"git -C {path} status"),
    ]:
        out = subprocess.run([sys.executable, "-I", str(HOOK)],
                             input=json.dumps({"tool_input": {"command": cmd}, "cwd": "/tmp"}),
                             capture_output=True, text=True).stdout
        got = "deny" in out
        print(("ok  " if got == want else "FAIL"), "deny " if got else "allow", "[builds]", cmd[:90])
        bad += got != want
else:
    print("skip end-to-end cases: `builds` records no app checkout")
sys.exit(bad)

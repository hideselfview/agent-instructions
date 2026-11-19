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
ANDROID_APP = "/Users/dima/dev/bae-worktrees/side-9"
CHECKOUTS = [(APP, "bae", "mac"), (ANDROID_APP, "bae", "android")]
ELSEWHERE = "/Users/dima/dev/bae-worktrees/side-2"

RECIPE = f"""builds lock bae sh -c '
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
    (True, ELSEWHERE, f"builds lock coven sh -c 'cd {APP} && git checkout x'"),
    (True, ELSEWHERE, f"lockf -k /Volumes/Builds/bae/app.lock sh -c 'cd {APP} && git checkout x'"),
    (True, ELSEWHERE, f"builds lock bae true; cd {APP} && git checkout x"),
    # The recipe from CLAUDE.md, and the same checkout moved under the lock.
    (False, ELSEWHERE, RECIPE),
    (False, ELSEWHERE, f"builds lock bae git -C {APP} checkout x"),
    (False, ELSEWHERE, f"~/.local/bin/builds lock bae git -C {APP} checkout x"),
    (False, "/Users/dima/dev/bae", RECIPE),
    (True, ELSEWHERE, f"cd {APP} && git commit -m x"),
    (True, ELSEWHERE, f"git -C {APP} add -A"),
    (True, ELSEWHERE, f"cd {APP} && git rm old.txt"),
    (True, ELSEWHERE, f"cd {APP} && git mv a b"),
    (True, ELSEWHERE, f"cd {APP} && git apply fix.patch"),
    (True, ELSEWHERE, f"cd {APP} && git update-ref refs/heads/x HEAD"),
    (True, ELSEWHERE, f"cd {APP} && git symbolic-ref HEAD refs/heads/x"),
    (True, ELSEWHERE, f"cd {APP} && git worktree add ../x"),
    (True, ELSEWHERE, f"git -C {APP} -c user.name=x commit -m x"),
    (False, ELSEWHERE, f"cd {APP} && git symbolic-ref HEAD && git worktree list"),
    (False, ELSEWHERE, f"cd {APP} && git commit --dry-run && git add -n ."),
    (False, ELSEWHERE, f"builds lock bae sh -c 'cd {APP} && git commit -m x'"),
    # A lane's lock covers its own checkout only.
    (True, ELSEWHERE, f"builds lock bae sh -c 'cd {ANDROID_APP} && git checkout x'"),
    (True, ELSEWHERE, f"builds lock bae --lane mac sh -c 'cd {ANDROID_APP} && git checkout x'"),
    (True, ELSEWHERE, f"builds lock bae --lane android sh -c 'cd {APP} && git checkout x'"),
    (True, ELSEWHERE, f"git -C {ANDROID_APP} checkout x"),
    (False, ELSEWHERE, f"builds lock bae --lane android sh -c 'cd {ANDROID_APP} && git checkout x'"),
    (False, ELSEWHERE, f"builds lock bae --lane android git -C {ANDROID_APP} checkout x"),
    (False, ELSEWHERE, f"builds lock bae sh -c 'cd {APP} && git checkout x'"),
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

# End to end through the hook's stdin protocol, with a `builds` that records one app checkout and, as the real
# one does, lists it for `status --json --no-folders` only.
import os
import stat
import tempfile

with tempfile.TemporaryDirectory() as folder:
    fake = Path(folder) / "builds"
    fake.write_text(f"""#!/bin/sh
case "$*" in
  "status --json --no-folders") echo '{json.dumps({"repos": [{"name": "bae", "checkouts": [
      {"path": APP, "app_checkout": True, "app_lane": "mac"}]}]})}' ;;
  *) echo "builds: has not acknowledged" >&2; exit 1 ;;
esac
""")
    fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
    env = {"PATH": f"{folder}:{os.environ['PATH']}"}
    lock = "builds lock bae"

    def run(cmd, **ids):
        return subprocess.run([sys.executable, "-I", str(HOOK)], env=env, capture_output=True, text=True,
                              input=json.dumps({"tool_input": {"command": cmd}, "cwd": "/tmp", **ids}))

    for want, cmd, ids in [
        (True, f"cd {APP} && git checkout -q --detach origin/main", {"session_id": "agent-7"}),
        (True, f"cd {APP} && git checkout -q --detach origin/main", {}),
        (False, f"{lock} sh -c 'cd {APP} && git checkout -q --detach x'", {"session_id": "agent-7"}),
        (False, f"git -C {APP} status", {"session_id": "agent-7"}),
    ]:
        out = run(cmd, **ids).stdout
        got = "deny" in out
        print(("ok  " if got == want else "FAIL"), "deny " if got else "allow", "[builds]", cmd[:90])
        bad += got != want

# When `builds` cannot be run, the hook cannot tell the app checkouts: it refuses every git change, says why, and
# lets other commands through without a word.
def without_builds(cmd):
    return subprocess.run([sys.executable, "-I", str(HOOK)], env={"PATH": "/nonexistent"},
                          input=json.dumps({"tool_input": {"command": cmd}, "cwd": "/tmp"}),
                          capture_output=True, text=True)

for want, cmd in [
    (True, f"cd {APP} && git checkout x"),
    (True, "git checkout -b x origin/main"),
    (True, f"builds lock bae sh -c 'cd {APP} && git checkout x'"),
    (False, "git status --short"),
    (False, "echo digit"),
]:
    broken = without_builds(cmd)
    got = "deny" in broken.stdout
    ok = got == want and broken.returncode == 0 and (not want or "builds status --json" in broken.stdout)
    print("ok  " if ok else "FAIL", "deny " if got else "allow", "[no builds]", cmd[:90])
    bad += not ok
sys.exit(bad)

"""Cases for no-skipped-hooks-or-hard-resets.py: python3 scripts/hooks/test_no_skipped_hooks_or_hard_resets.py"""

import json
import subprocess
import sys
from pathlib import Path

HOOK = str(Path(__file__).with_name("no-skipped-hooks-or-hard-resets.py"))
CASES = [
    (True, 'git commit -q --no-verify -m "wip probe"'),
    (True, 'git commit -n -m x'),
    (True, 'git commit -anm x'),
    (True, 'git commit -m x -n'),
    (True, 'git commit --amend --no-edit --no-verify'),
    (True, 'git push --no-verify origin main'),
    (True, 'git push origin HEAD --no-verify'),
    (True, 'git -C /tmp/w commit --no-verify -m x'),
    (True, 'git -c user.name=a commit -n -m x'),
    (True, 'git merge --no-verify topic'),
    (True, 'git rebase --no-verify main'),
    (True, 'git reset --hard'),
    (True, 'git reset -q --hard HEAD~1'),
    (True, 'git reset HEAD~1 --hard'),
    (True, 'git -C /tmp/w reset --hard origin/main'),
    (True, 'cd w && git add f && git commit --no-verify -m x'),
    (True, 'echo a\ngit reset --hard'),
    (True, 'git status; git reset --hard'),
    (True, 'FOO=1 git commit -n -m x'),
    (True, 'env GIT_AUTHOR_NAME=a git commit --no-verify -m x'),
    (True, 'timeout 60 git push --no-verify'),
    (True, '/usr/bin/git reset --hard'),
    (False, 'git commit -m "fix: wire --no-verify through"'),
    (False, 'git commit -m "git reset --hard is refused"'),
    (False, 'git commit -m n'),
    (False, 'git commit -mnote'),
    (False, 'git commit -am "x"'),
    (False, 'git commit -q -m x'),
    (False, 'git commit -F msg.txt'),
    (False, 'git push -n origin main'),
    (False, 'git push --dry-run'),
    (False, 'git reset --soft HEAD~1'),
    (False, 'git reset HEAD file'),
    (False, 'git reset --mixed'),
    (False, 'git status'),
    (False, 'git log --oneline -n 5'),
    (False, 'git diff -n'),
    (False, 'git checkout -- f --hard'),
    (False, 'echo git reset --hard'),
    (False, 'grep -n "no-verify" notes.txt'),
    (False, 'builds land w --done 1 abc'),
    (False, 'git commit -m "x" -- --no-verify'),
]
bad = 0
for want, cmd in CASES:
    out = subprocess.run([sys.executable, "-I", HOOK], input=json.dumps({"tool_input": {"command": cmd}}),
                         capture_output=True, text=True).stdout
    got = "deny" in out
    print(("ok  " if got == want else "FAIL"), "deny " if got else "allow", cmd.replace("\n", " "))
    bad += got != want
sys.exit(bad)

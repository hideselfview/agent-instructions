"""Cases for land.sh's whole-tree checks: python3 scripts/land/test_land.py

Builds a bare origin and a detached landing clone in a temp dir. The repo's
.land-checks refuses a tree holding both a.txt and b.txt, which two commits that
each pass alone reach only together.
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

LAND = str(Path(__file__).with_name("land.sh"))
ENV = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t",
       "GIT_COMMITTER_EMAIL": "t@t", "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null"}


def git(cwd, *args):
    return subprocess.run(["git", *args], cwd=cwd, env=ENV, check=True, capture_output=True, text=True).stdout.strip()


def commit_file(cwd, name, text, msg):
    Path(cwd, name).write_text(text)
    git(cwd, "add", name)
    git(cwd, "commit", "-q", "-m", msg)
    return git(cwd, "rev-parse", "HEAD")


def fixture(root, checks):
    """An origin with one commit, an agent clone, and a detached landing clone."""
    origin = f"{root}/origin.git"
    git(root, "init", "-q", "--bare", "-b", "main", origin)
    agent = f"{root}/agent"
    git(root, "clone", "-q", origin, agent)
    Path(agent, ".land-checks").write_text(checks)
    commit_file(agent, "base.txt", "base\n", "base")
    git(agent, "add", ".land-checks")
    git(agent, "commit", "-q", "--amend", "--no-edit")
    git(agent, "push", "-q", "origin", "HEAD:main")
    land = f"{root}/land"
    git(root, "clone", "-q", origin, land)
    git(land, "checkout", "-q", "--detach", "origin/main")
    return origin, agent, land


def run_land(land, *shas):
    p = subprocess.run(["bash", LAND, land, *shas], env=ENV, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


def publish(agent, name, sha):
    """Make a commit fetchable from the landing clone, as a shared object store does for worktrees."""
    git(agent, "push", "-q", "origin", f"{sha}:refs/heads/{name}")


def two_sibling_commits(agent, land):
    """Commits a.txt and b.txt on the base, published for landing."""
    base = git(agent, "rev-parse", "HEAD")
    a = commit_file(agent, "a.txt", "a\n", "add a")
    git(agent, "checkout", "-q", "--detach", base)
    b = commit_file(agent, "b.txt", "b\n", "add b")
    publish(agent, "a", a)
    publish(agent, "b", b)
    return a, b


CHECK = "! { test -e a.txt && test -e b.txt; } || { echo 'a.txt and b.txt together'; exit 1; }\n"
bad = 0


def expect(name, ok):
    global bad
    print("ok  " if ok else "FAIL", name)
    bad += not ok


with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "# comment\n\n" + CHECK)
    a, b = two_sibling_commits(agent, land)
    # a lands by fast-forward (base is origin/main); the checks do not run.
    code, out = run_land(land, a)
    expect("fast-forward lands without checks", code == 0 and git(origin, "rev-parse", "main") == a)
    # b's base is behind origin/main, so it is cherry-picked; a+b together fail the check.
    code, out = run_land(land, b)
    expect("cherry-picked result failing a check is refused", code == 1 and "CHECK FAILED" in out and "a.txt and b.txt together" in out)
    expect("origin unchanged after refusal", git(origin, "rev-parse", "main") == a)
    expect("landing worktree back at origin/main and clean",
           git(land, "rev-parse", "HEAD") == a and git(land, "status", "--porcelain") == "")

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n")
    a, b = two_sibling_commits(agent, land)
    run_land(land, a)
    code, out = run_land(land, b)
    expect("cherry-picked result passing the checks is pushed",
           code == 0 and git(origin, "show", "main:b.txt") == "b" and git(origin, "show", "main:a.txt") == "a")

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, CHECK)
    a, b = two_sibling_commits(agent, land)
    # Both at once, based on origin/main: a fast-forward chain, so the failing check never runs.
    git(agent, "checkout", "-q", "--detach", a)
    c = commit_file(agent, "b.txt", "b\n", "add b on a")
    publish(agent, "c", c)
    code, out = run_land(land, a, c)
    expect("fast-forward of a failing tree skips the checks", code == 0 and git(origin, "rev-parse", "main") == c)

sys.exit(bad)

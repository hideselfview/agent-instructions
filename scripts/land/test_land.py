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


def fixture(root, checks, compile_check=None):
    """An origin with one commit, an agent clone, and a detached landing clone."""
    origin = f"{root}/origin.git"
    git(root, "init", "-q", "--bare", "-b", "main", origin)
    agent = f"{root}/agent"
    git(root, "clone", "-q", origin, agent)
    Path(agent, ".land-checks").write_text(checks)
    commit_file(agent, "base.txt", "base\n", "base")
    git(agent, "add", ".land-checks")
    if compile_check is not None:
        Path(agent, ".land-compile").write_text(compile_check)
        git(agent, "add", ".land-compile")
    git(agent, "commit", "-q", "--amend", "--no-edit")
    git(agent, "push", "-q", "origin", "HEAD:main")
    land = f"{root}/land"
    git(root, "clone", "-q", origin, land)
    git(land, "checkout", "-q", "--detach", "origin/main")
    return origin, agent, land


# A stand-in `builds` first on PATH for every landing, so no test reaches the real one.
# It appends each call to $STUB_LOG and answers by cue: STUB_PROJECT is what
# `lists project-of` prints (nothing when unset), STUB_PLACES what `design places`
# prints (`place`, `source`, `spec` or `shared` lines), STUB_REFUSE the "<group> <command>" it exits 1 on,
# saying STUB_SAYS when that is set.
STUB = """#!/bin/bash
echo "$*" >> "$STUB_LOG"
[ "$STUB_REFUSE" = "$1 $2" ] && { echo "${STUB_SAYS:-stub refuses $1 $2}"; exit 1; }
case "$1 $2" in
  "lists project-of") echo "$STUB_PROJECT" ;;
  "design places") echo "$STUB_PLACES" ;;
esac
exit 0
"""


def run_land(land, *args, **cues):
    stubdir = Path(land).parent / "stub"
    stubdir.mkdir(exist_ok=True)
    (stubdir / "builds").write_text(STUB)
    (stubdir / "builds").chmod(0o755)
    log = stubdir / "calls.log"
    env = {**ENV, "PATH": f"{stubdir}:{ENV['PATH']}", "STUB_LOG": str(log), **cues}
    p = subprocess.run(["bash", LAND, land, *args], env=env, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


def calls(land):
    log = Path(land).parent / "stub" / "calls.log"
    return log.read_text().splitlines() if log.exists() else []


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
    # a lands by fast-forward (base is origin/main); a alone passes the check.
    code, out = run_land(land, a)
    expect("fast-forward of a passing tree lands", code == 0 and git(origin, "rev-parse", "main") == a)
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
    # Both at once, based on origin/main: a fast-forward chain, which runs the checks too.
    git(agent, "checkout", "-q", "--detach", a)
    c = commit_file(agent, "b.txt", "b\n", "add b on a")
    publish(agent, "c", c)
    code, out = run_land(land, a, c)
    expect("fast-forward of a failing tree is refused by the checks",
           code == 1 and "CHECK FAILED" in out and git(origin, "rev-parse", "main") != c)

COMPILE = "#!/bin/bash\n! { test -e a.txt && test -e b.txt; } || { echo 'a.txt and b.txt do not compile together'; exit 1; }\n"

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n", COMPILE)
    a, b = two_sibling_commits(agent, land)
    run_land(land, a)
    code, out = run_land(land, b)
    expect("cherry-picked result failing the compile check is refused",
           code == 1 and "COMPILE CHECK FAILED" in out and "do not compile together" in out)
    expect("origin unchanged after compile refusal", git(origin, "rev-parse", "main") == a)
    expect("landing worktree clean at origin/main after compile refusal",
           git(land, "rev-parse", "HEAD") == a and git(land, "status", "--porcelain") == "")

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n", "#!/bin/bash\ntrue\n")
    a, b = two_sibling_commits(agent, land)
    run_land(land, a)
    code, out = run_land(land, b)
    expect("cherry-picked result passing the compile check is pushed",
           code == 0 and git(origin, "show", "main:b.txt") == "b")

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n", COMPILE)
    a, b = two_sibling_commits(agent, land)
    git(agent, "checkout", "-q", "--detach", a)
    c = commit_file(agent, "b.txt", "b\n", "add b on a")
    publish(agent, "c", c)
    code, out = run_land(land, a, c)
    expect("fast-forward runs the compile check",
           code == 1 and "COMPILE CHECK FAILED" in out and git(origin, "rev-parse", "main") != c)

# The lists: a repo in a project names its work items; one in no project lands as before.
with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n")
    a, b = two_sibling_commits(agent, land)
    code, out = run_land(land, a)
    expect("repo in no project lands with only the project lookup",
           code == 0 and git(origin, "rev-parse", "main") == a and calls(land) == [f"lists project-of {land}"])
    code, out = run_land(land, "--done", "5", b)
    expect("repo in no project refuses --done", code == 1 and "no project" in out and git(origin, "rev-parse", "main") == a)

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n")
    a, b = two_sibling_commits(agent, land)
    code, out = run_land(land, a, STUB_PROJECT="bae")
    expect("project repo without --done or --part is refused before anything moves",
           code == 1 and "--done" in out and git(origin, "rev-parse", "main") != a
           and calls(land) == [f"lists project-of {land}", f"design places {land} {a}"])
    code, out = run_land(land, "--done", "7", a, STUB_PROJECT="bae", STUB_REFUSE="item check-landing")
    expect("refused check-landing: nothing fetched, picked or pushed",
           code == 1 and "stub refuses" in out and git(origin, "rev-parse", "main") != a
           and git(land, "rev-parse", "HEAD") == git(agent, "rev-parse", "origin/main"))
    code, out = run_land(land, "--done", "7", "--part", "8", a, STUB_PROJECT="bae")
    expect("project repo lands and records main's commits",
           code == 0 and git(origin, "rev-parse", "main") == a
           and calls(land)[-1] == f"item landed {land} --done 7 --part 8 {a}"
           and not any(c.startswith("design landed") for c in calls(land)))

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n")
    a, b = two_sibling_commits(agent, land)
    run_land(land, a)
    code, out = run_land(land, "--done", "7", b, STUB_PROJECT="bae")
    new = git(origin, "rev-parse", "main")
    expect("a cherry-picked landing records the new commit's SHA on main, not the original",
           code == 0 and new != b and calls(land)[-1] == f"item landed {land} --done 7 {new}")

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n")
    a, b = two_sibling_commits(agent, land)
    code, out = run_land(land, "--done", "7", a, STUB_PROJECT="bae", STUB_PLACES="place design/mocks/button")
    expect("design places without --design are refused before anything moves",
           code == 1 and "--design" in out and git(origin, "rev-parse", "main") != a)
    code, out = run_land(land, "--done", "7", a, STUB_PROJECT="bae", STUB_PLACES="shared design/system/tokens")
    expect("shared-only design files without --design are refused",
           code == 1 and "--design" in out and git(origin, "rev-parse", "main") != a)
    code, out = run_land(land, "--done", "7", "--design", "3", a, STUB_PROJECT="bae")
    expect("--design with no design places is refused", code == 1 and "no design place" in out)
    code, out = run_land(land, "--done", "7", "--design", "3", a, STUB_PROJECT="bae",
                         STUB_PLACES="place design/mocks/button", STUB_REFUSE="design check")
    expect("refused design check: nothing pushed", code == 1 and git(origin, "rev-parse", "main") != a)
    code, out = run_land(land, "--done", "7", "--design", "3", a, STUB_PROJECT="bae", STUB_PLACES="place design/mocks/button")
    expect("design landing records the item and the design change",
           code == 0 and f"design check 3 {land} {a}" in calls(land)
           and calls(land)[-2:] == [f"item landed {land} --done 7 {a}", f"design landed 3 {land} {a}"])

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n")
    a, b = two_sibling_commits(agent, land)
    code, out = run_land(land, "--done", "7", "--design", "3", a, STUB_PROJECT="bae", STUB_PLACES="shared design/system/tokens")
    expect("shared-only design files with --design land and record the design change",
           code == 0 and calls(land)[-1] == f"design landed 3 {land} {a}")

# A design change is the whole landing when every commit's paths are design files.
with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n")
    a, b = two_sibling_commits(agent, land)
    code, out = run_land(land, "--design", "3", a, STUB_PROJECT="bae")
    expect("--design alone with no design place is refused",
           code == 1 and "no design place" in out and git(origin, "rev-parse", "main") != a)
    code, out = run_land(land, a, STUB_PROJECT="bae", STUB_PLACES="place design/mocks/button")
    expect("design places with neither --done nor --design are refused",
           code == 1 and "--done" in out and git(origin, "rev-parse", "main") != a)
    code, out = run_land(land, "--design", "3", a, STUB_PROJECT="bae", STUB_PLACES="place design/mocks/button",
                         STUB_REFUSE="design only", STUB_SAYS="the commits also change files that are no design file: src/x.rs")
    expect("--design alone refuses commits that are not all design files, nothing pushed",
           code == 1 and "src/x.rs" in out and "--done" in out and git(origin, "rev-parse", "main") != a
           and not any(c.startswith("design check") for c in calls(land)))
    code, out = run_land(land, "--design", "3", a, STUB_PROJECT="bae", STUB_PLACES="place design/mocks/button",
                         STUB_REFUSE="design check", STUB_SAYS="design change 3 waits on the person's review")
    expect("an unapproved design change is refused with its reason, nothing pushed",
           code == 1 and "waits on the person's review" in out and git(origin, "rev-parse", "main") != a
           and git(land, "rev-parse", "HEAD") == git(agent, "rev-parse", "origin/main"))
    code, out = run_land(land, "--design", "3", a, STUB_PROJECT="bae", STUB_PLACES="place design/mocks/button\nspec spec/bae.md#Sync")
    expect("--design alone lands all-design commits and records only the design change",
           code == 0 and git(origin, "rev-parse", "main") == a
           and f"design only {land} {a}" in calls(land) and f"design check 3 {land} {a}" in calls(land)
           and calls(land)[-1] == f"design landed 3 {land} {a}"
           and not any(c.startswith("item ") for c in calls(land)))

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n")
    a, b = two_sibling_commits(agent, land)
    code, out = run_land(land, "--design", "3", a, STUB_PROJECT="bae", STUB_PLACES="place design/mocks/button", STUB_REFUSE="design landed")
    expect("a failed design record after a design-only push exits 1 with the rerun",
           code == 1 and git(origin, "rev-parse", "main") == a and "LANDED, NOT RECORDED:" in out
           and f"rerun: builds design landed 3 {land} {a}" in out and "item landed" not in out)

with tempfile.TemporaryDirectory() as root:
    origin, agent, land = fixture(root, "true\n")
    a, b = two_sibling_commits(agent, land)
    code, out = run_land(land, "--done", "7", a, STUB_PROJECT="bae", STUB_REFUSE="item landed")
    expect("failed record after the push exits 1 with the rerun",
           code == 1 and git(origin, "rev-parse", "main") == a and "LANDED, NOT RECORDED:" in out
           and f"rerun: builds item landed {land} --done 7 {a}" in out)
    code, out = run_land(land, "--done", "7", a, STUB_PROJECT="bae")
    expect("rerunning the landing after a failed record records it (already on main)",
           code == 0 and calls(land)[-1] == f"item landed {land} --done 7 {a}")

sys.exit(bad)

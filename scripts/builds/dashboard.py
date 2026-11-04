#!/usr/bin/env python3
"""A page in the browser showing what every checkout `builds` knows is doing.

    dashboard.py <builds> <port>   serve it on 127.0.0.1:<port> until killed

<builds> is the `builds` script, which this asks for everything it knows:
`status --json --no-folders` for the checkouts and their claims, and
`status --json` for their build folders' sizes and last writes. Git is asked
only about side worktrees, read-only and with --no-optional-locks: HEAD, the
commits on HEAD whose change the local origin/main ref doesn't have, and the
count of uncommitted files. Main checkouts are the person's; for them the page
shows only what `builds status` does. Nothing is fetched.

Each side worktree gets one state, from its claim and git (worktree_state):
waiting-on-you, landing, ready-to-land, winding-down, working or free. The
page has two views of them, cards by repo and lanes by state, and keeps the
viewer's choice in the browser.

/api/state reads fresh on every request and keeps nothing. Measuring the build
folders walks the disk for most of half a minute, so one background thread
owns it (Measurement): it runs `status --json`, keeps the last result with the
time it finished, waits 5 minutes, and runs again, but only while /api/state or
/api/folders was asked for in the last 2 minutes: with no page open, nothing
walks the disk. A request that finds the last result older than 5 minutes
starts a run at once. No request waits on a measurement.

    /                the page; it asks /api/state every few seconds and
                     /api/folders every minute, and nothing while its tab is
                     hidden
    /api/state       the volume's free space and floors, each repo's app.lock
                     and its holder, and each checkout with its claim and git
    /api/folders     the last measurement of each checkout's build folders,
                     with sizes and generated_at, the time it finished; until
                     the first one finishes, {"measuring": true}

A request whose Host isn't this server's own address is refused, so a page
from elsewhere that resolves its name to 127.0.0.1 can't read these.
"""

import concurrent.futures
import http.server
import json
import os
import subprocess
import sys
import threading
import time

GIT = ["git", "--no-optional-locks"]
# Run git and builds from a folder no repo contains, with git's optional
# locks off however git is reached.
ENV = dict(os.environ, GIT_OPTIONAL_LOCKS="0")
CWD = "/"


class Failed(Exception):
    """A command that failed, with what it printed."""


def run(argv):
    done = subprocess.run(argv, cwd=CWD, env=ENV, capture_output=True, text=True)
    if done.returncode != 0:
        raise Failed(f"{' '.join(argv[:6])} exited {done.returncode}: {done.stderr.strip()}")
    return done.stdout


def builds_status(builds, folders):
    argv = [builds, "status", "--json"] + ([] if folders else ["--no-folders"])
    return json.loads(run(argv))


def commits(top, *rev):
    """The commits `git log <rev>` lists in <top>, newest first."""
    out = run(GIT + ["-C", top, "log", "--format=%h%x00%ct%x00%s%x00", *rev])
    fields = out.split("\0")
    found = []
    for i in range(0, len(fields) - 2, 3):
        found.append({"sha": fields[i].strip(), "time": int(fields[i + 1]), "subject": fields[i + 2]})
    return found


def uncommitted(top):
    """The count of changed and untracked files in <top>."""
    out = run(GIT + ["-C", top, "status", "--porcelain", "-z"])
    entries = out.split("\0")
    count, i = 0, 0
    while i < len(entries):
        entry = entries[i]
        i += 1
        if not entry:
            continue
        count += 1
        # A rename or copy names its source in the entry after it.
        if entry[0] in "RC" or entry[1] in "RC":
            i += 1
    return count


def side_git(top):
    """HEAD, the commits waiting to land and the uncommitted count of the side
    worktree at <top>; on a failure, what git said.

    A commit waits to land when origin/main has no commit with the same change:
    a coordinator lands a branch by rebasing or cherry-picking it, so the
    branch's own commits stay off origin/main by hash after they land, and
    `origin/main..HEAD` would list landed work as waiting."""
    try:
        head = commits(top, "-1", "HEAD")
        return {
            "head": head[0] if head else None,
            "ahead": commits(top, "--cherry-pick", "--right-only", "origin/main...HEAD"),
            "uncommitted": uncommitted(top),
            "error": None,
        }
    except Failed as e:
        return {"head": None, "ahead": [], "uncommitted": None, "error": str(e)}


def lock_state(path):
    """Who holds the lock file at <path> and how many wait for it, read
    without taking it. `lockf` opens the file with O_EXLOCK, which waits
    inside open(): the one process that has the file open holds it, and a
    lockf naming the file without having it open waits."""
    found = subprocess.run(["lsof", "-nP", "-Fp", "--", path], cwd=CWD, capture_output=True, text=True)
    holders = {int(line[1:]) for line in found.stdout.splitlines() if line.startswith("p")}
    processes = []
    for line in run(["ps", "-A", "-ww", "-o", "pid=,ppid=,command="]).splitlines():
        parts = line.split(None, 2)
        if len(parts) == 3:
            processes.append((int(parts[0]), int(parts[1]), parts[2]))
    held = []
    for pid in sorted(holders):
        command = next((c for p, _, c in processes if p == pid), "")
        children = [c for _, parent, c in processes if parent == pid]
        held.append({"pid": pid, "command": command, "running": children})
    waiting = 0
    for pid, _, command in processes:
        words = command.split()
        if pid not in holders and words and os.path.basename(words[0]) == "lockf" and path in words:
            waiting += 1
    return {"holders": held, "waiting": waiting}


def worktree_state(checkout):
    """The one state the page shows for the live side worktree <checkout>,
    first match wins:

        waiting-on-you  its note says so: a question or step only the person
                        can take
        landing         its claim's who ends in "(landing)": the worktree a
                        coordinator lands commits from, whose commits off
                        origin/main are a landing under way
        ready-to-land   commits waiting to land, claimed or not: unclaimed
                        ones are work an agent left behind
        winding-down    its note says so: the agent's last item, then its
                        handoff
        working         claimed, none of the above
        free            unclaimed, nothing waiting to land
    """
    claim = checkout.get("claim")
    git = checkout.get("git")
    ahead = bool(git and git["ahead"])
    if not claim:
        return "ready-to-land" if ahead else "free"
    if claim["note_state"] == "waiting-on-you":
        return "waiting-on-you"
    if claim["who"].endswith("(landing)"):
        return "landing"
    if ahead:
        return "ready-to-land"
    if claim["note_state"] == "winding-down":
        return "winding-down"
    return "working"


def state(builds):
    started = time.time()
    status = builds_status(builds, folders=False)
    sides = [c for r in status["repos"] for c in r["checkouts"] if c["live"] and not c["main"]]
    locks = [r["app_lock"] for r in status["repos"] if r["app_lock"]]
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        gits = dict(zip([c["path"] for c in sides], pool.map(side_git, [c["path"] for c in sides])))
        held = dict(zip(locks, pool.map(lock_state, locks)))
    for repo in status["repos"]:
        repo["lock"] = held.get(repo["app_lock"])
        for checkout in repo["checkouts"]:
            checkout["git"] = gits.get(checkout["path"])
            side = checkout["live"] and not checkout["main"]
            checkout["state"] = worktree_state(checkout) if side else None
    status["generated_at"] = int(time.time())
    status["took_seconds"] = round(time.time() - started, 1)
    return status


def measure_folders(builds):
    started = time.time()
    status = builds_status(builds, folders=True)
    status["generated_at"] = int(time.time())
    status["took_seconds"] = round(time.time() - started, 1)
    return status


class Measurement:
    """The build folders' sizes, measured by one background thread, never by a
    request. A run starts when someone has asked for the page's data in the
    last `window` seconds and the last run finished `interval` or more seconds
    ago (or none has finished); runs never overlap, since the one thread
    makes them."""

    def __init__(self, measure, interval=300.0, window=120.0):
        self.measure = measure
        self.interval = interval
        self.window = window
        self.cond = threading.Condition()
        self.wanted_at = None  # monotonic time of the last request
        self.finished_at = None  # monotonic time the last run ended, either way
        self.result = None
        self.error = None
        threading.Thread(target=self.loop, daemon=True).start()

    def want(self):
        """Note a request for the page's data, waking the thread."""
        with self.cond:
            self.wanted_at = time.monotonic()
            self.cond.notify_all()

    def latest(self):
        """The last result, {"measuring": true} before the first run ends, or
        raise Failed when the runs so far have all failed. Never waits."""
        with self.cond:
            result, error = self.result, self.error
        if result is None:
            if error:
                raise Failed(error)
            return {"measuring": True}
        return dict(result, error=error)

    def next_wait(self):
        """Seconds until a run is due, 0 when one is, None when nobody wants
        one. Called with the lock held."""
        now = time.monotonic()
        if self.wanted_at is None or now - self.wanted_at >= self.window:
            return None
        if self.finished_at is None:
            return 0
        return max(0.0, self.interval - (now - self.finished_at))

    def loop(self):
        while True:
            with self.cond:
                wait = self.next_wait()
                while wait != 0:
                    self.cond.wait(wait)
                    wait = self.next_wait()
            try:
                result, error = self.measure(), None
            except (Failed, ValueError) as e:
                result, error = None, str(e)
            with self.cond:
                if error is None:
                    self.result = result
                self.error = error
                self.finished_at = time.monotonic()


def handler(builds, port):
    measurement = Measurement(lambda: measure_folders(builds))
    hosts = {f"127.0.0.1:{port}", f"localhost:{port}"}

    class Handler(http.server.BaseHTTPRequestHandler):
        def send(self, code, kind, body):
            data = body.encode()
            self.send_response(code)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.headers.get("Host") not in hosts:
                self.send(403, "text/plain; charset=utf-8", "only for this machine's own browser\n")
                return
            routes = {"/api/state": lambda: state(builds), "/api/folders": measurement.latest}
            path = self.path.split("?", 1)[0]
            if path == "/":
                self.send(200, "text/html; charset=utf-8", PAGE)
            elif path in routes:
                measurement.want()
                try:
                    body = json.dumps(routes[path]())
                except (Failed, ValueError) as e:
                    self.send(500, "application/json", json.dumps({"error": str(e)}))
                    return
                self.send(200, "application/json", body)
            else:
                self.send(404, "text/plain; charset=utf-8", "not found\n")

        def log_message(self, format, *args):
            pass

    return Handler


def main(argv):
    if len(argv) != 2 or not argv[1].isdigit():
        print("usage: dashboard.py <builds> <port>", file=sys.stderr)
        return 2
    builds, port = argv[0], int(argv[1])
    try:
        server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler(builds, port))
    except OSError as e:
        print(f"builds: dashboard: can't listen on 127.0.0.1:{port}: {e.strerror}", file=sys.stderr)
        return 1
    print(f"builds: dashboard at http://127.0.0.1:{port}/ (pid {os.getpid()})", file=sys.stderr, flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


PAGE = r"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Builds</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap">
<style>
:root {
  --sans: Inter, system-ui, -apple-system, "Segoe UI", sans-serif;
  --mono: "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, monospace;
  --bg: #F6F6F3; --surface: #FFFFFF; --pinned: #EDEDE8; --pinned-line: #DADAD3;
  --line: #E2E2DC; --line-soft: #F0F0EA; --track: #ECECE6;
  --text: #1A1A19; --text-2: #2B2B29; --text-3: #3A3A37; --muted: #5C5C57; --muted-2: #6B6B66;
  --ok: #2E7D4F; --warn: #9A5B00; --bad: #B3261E; --bad-bg: #FDE7E5;
  --accent: #1F4FB8; --waiting-bg: #EEF3FC;
  --working-bg: #E7F2EA; --working: #1F5E38;
  --ready-bg: #E3ECFB; --ready: #1F4FB8;
  --you-bg: #FBEBDD; --you: #8A3E08;
  --winding-bg: #EFEDE6; --winding: #4F4C42;
  --free-bg: #F1F1EC; --free: #5C5C57;
  --dot-you: #C25E10; --dot-ready: #1F4FB8; --dot-working: #2E7D4F; --dot-free: #9A9A92;
  --on: #1A1A19; --on-text: #FFFFFF;
  color-scheme: light;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #161615; --surface: #1F1F1D; --pinned: #262624; --pinned-line: #383833;
    --line: #34342F; --line-soft: #2B2B28; --track: #34342F;
    --text: #ECECE8; --text-2: #D9D9D3; --text-3: #C9C9C2; --muted: #A6A69F; --muted-2: #9C9C95;
    --ok: #5DBB84; --warn: #E8A84A; --bad: #FF8A80; --bad-bg: #3D1A17;
    --accent: #93B2FF; --waiting-bg: #1C2538;
    --working-bg: #1C3324; --working: #93D8AE;
    --ready-bg: #1D2A45; --ready: #A9C3FF;
    --you-bg: #3E2614; --you: #F6B27C;
    --winding-bg: #2D2B25; --winding: #D3CEBD;
    --free-bg: #2A2A27; --free: #B5B5AE;
    --dot-you: #E07A2E; --dot-ready: #6F95F0; --dot-working: #4FAE76; --dot-free: #85857E;
    --on: #ECECE8; --on-text: #161615;
    color-scheme: dark;
  }
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--text); font-family: var(--sans); }
.page { padding: 24px 28px 48px; display: flex; flex-direction: column; gap: 20px; min-height: 100vh; }
.mono { font-family: var(--mono); }

header { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 16px; }
.title { display: flex; align-items: baseline; gap: 14px; flex-wrap: wrap; min-width: 0; }
h1 { margin: 0; font-size: 22px; font-weight: 700; letter-spacing: -0.01em; }
.sub { font-size: 14px; color: var(--muted); }
.controls { display: flex; flex-wrap: wrap; gap: 10px; align-items: center; }
.switch { display: inline-flex; gap: 2px; padding: 3px; background: var(--surface); border: 1px solid var(--line); border-radius: 10px; }
.switch button { font: 600 13px var(--sans); color: var(--muted); background: transparent; border: 0; border-radius: 7px; padding: 5px 12px; cursor: pointer; }
.switch button[aria-pressed="true"] { background: var(--on); color: var(--on-text); }
.switch button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.disk { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; background: var(--surface); border: 1px solid var(--line); border-radius: 10px; padding: 8px 12px; }
.disk .label { font-size: 13px; color: var(--muted); }
.bar { position: relative; width: 140px; height: 8px; background: var(--track); border-radius: 4px; overflow: hidden; }
.bar .fill { position: absolute; left: 0; top: 0; bottom: 0; background: var(--ok); }
.bar .mark { position: absolute; top: 0; bottom: 0; width: 2px; background: var(--muted-2); }
.disk .free { font: 500 13px var(--mono); }
.disk .floors, .updated { font-size: 12px; color: var(--muted-2); }
.disk .floors.warn { color: var(--warn); }
.disk .floors.bad { color: var(--bad); }
.error { background: var(--bad-bg); color: var(--bad); border-radius: 8px; padding: 8px 10px; font-size: 13px; white-space: pre-wrap; overflow-wrap: anywhere; }
.loading { font-size: 14px; color: var(--muted); }
#view { display: flex; flex-direction: column; gap: 28px; }

section.repo { display: flex; flex-direction: column; gap: 12px; }
.repo-head { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 10px; border-bottom: 1px solid var(--line); padding-bottom: 8px; }
.repo-head h2 { margin: 0; font-size: 16px; font-weight: 700; }
.repo-head span { font-size: 13px; color: var(--muted); }
.pinned-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(420px, 100%), 1fr)); gap: 12px; align-items: start; }
.cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(320px, 100%), 1fr)); gap: 12px; }
article { min-width: 0; display: flex; flex-direction: column; }
.pinned { background: var(--pinned); border: 1px solid var(--pinned-line); border-radius: 12px; padding: 14px 16px; gap: 10px; }
.card { background: var(--surface); border: 1px solid var(--line); border-radius: 12px; padding: 14px 16px; gap: 10px; }
.row1 { display: flex; align-items: center; justify-content: space-between; gap: 8px; min-width: 0; }
.ident { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
.name { font-size: 15px; font-weight: 700; white-space: nowrap; }
.where { font: 12px var(--mono); color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.pill { font-size: 12px; font-weight: 600; padding: 3px 9px; border-radius: 999px; white-space: nowrap; flex: none; }
.s-working { background: var(--working-bg); color: var(--working); }
.s-ready { background: var(--ready-bg); color: var(--ready); }
.s-you { background: var(--you-bg); color: var(--you); }
.s-winding { background: var(--winding-bg); color: var(--winding); }
.s-free { background: var(--free-bg); color: var(--free); }
.line { font-size: 14px; line-height: 1.4; color: var(--text-2); overflow-wrap: anywhere; }
.total { display: flex; align-items: baseline; gap: 10px; }
.total .big { font: 500 20px var(--mono); }
.total .of { font-size: 12px; color: var(--muted); }
.chips { display: flex; flex-wrap: wrap; gap: 6px 8px; }
.chip { font-size: 12px; background: var(--surface); border: 1px solid var(--line); border-radius: 6px; padding: 3px 8px; color: var(--text-3); overflow-wrap: anywhere; }
.chip .mono { color: var(--text); }
.chip.over { border-color: var(--bad); }
.chip.over .mono { color: var(--bad); }
.command { font: 12px/1.4 var(--mono); background: var(--surface); border: 1px solid var(--line); border-radius: 6px; padding: 6px 8px; color: var(--text); overflow-wrap: anywhere; white-space: pre-wrap; }
.who-block { display: flex; flex-direction: column; gap: 3px; }
.who { font-size: 14px; font-weight: 600; overflow-wrap: anywhere; }
.note { font-size: 14px; line-height: 1.4; color: var(--text-2); overflow-wrap: anywhere; }
.waiting { background: var(--waiting-bg); border-radius: 8px; padding: 8px 10px; display: flex; flex-direction: column; gap: 4px; }
.waiting .label { font-size: 12px; font-weight: 600; color: var(--accent); }
.commit { display: flex; gap: 8px; font-size: 13px; line-height: 1.35; min-width: 0; }
.commit .sha { font-family: var(--mono); color: var(--accent); flex: none; }
.commit .subject { color: var(--text-2); overflow-wrap: anywhere; min-width: 0; }
.more { font-size: 12px; color: var(--muted); }
.foot { display: flex; flex-wrap: wrap; gap: 6px 14px; font-size: 12px; color: var(--muted); border-top: 1px solid var(--line-soft); padding-top: 8px; margin-top: auto; }
.bad { color: var(--bad); font-weight: 600; }
.warn { color: var(--warn); font-weight: 600; }

.pinned-row { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(300px, 100%), 1fr)); gap: 12px; align-items: start; }
.pinned.compact { padding: 12px 14px; gap: 6px; }
.compact .row1 { align-items: baseline; }
.compact .name { font-size: 14px; white-space: normal; }
.compact .big { font: 500 16px var(--mono); white-space: nowrap; }
.compact .line { font-size: 13px; color: var(--text-3); }
.lanes { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(280px, 100%), 1fr)); gap: 16px; align-items: start; }
.lane { background: var(--pinned); border-radius: 14px; padding: 12px; display: flex; flex-direction: column; gap: 10px; min-width: 0; }
.lane-head { display: flex; align-items: center; justify-content: space-between; padding: 2px 4px; }
.lane-head > div { display: flex; align-items: center; gap: 8px; }
.dot { width: 10px; height: 10px; border-radius: 50%; flex: none; }
.lane h2 { margin: 0; font-size: 14px; font-weight: 700; }
.lane .count { font: 13px var(--mono); color: var(--muted); }
.lane .hint, .lane .none { font-size: 12px; color: var(--muted); padding: 0 4px; }
.lcard { background: var(--surface); border: 1px solid var(--line); border-radius: 10px; padding: 12px 14px; gap: 8px; }
.lcard .row1 { align-items: baseline; }
.lcard .name { font-size: 14px; }
.lcard .age { font-size: 12px; color: var(--muted-2); white-space: nowrap; }
.lcard .who { font-size: 13px; color: var(--text-3); }
.lcard .note { font-size: 13px; color: var(--text); }
.lcard .commits { display: flex; flex-direction: column; gap: 3px; border-top: 1px solid var(--line-soft); padding-top: 6px; }
.lcard .commit { font-size: 12px; }
.lcard .pill { align-self: flex-start; font-size: 11px; padding: 2px 8px; }

@media (max-width: 600px) {
  .page { padding: 16px 16px 40px; gap: 16px; }
  .bar { width: 100px; }
}
</style>
</head>
<body>
<div class="page">
<header>
  <div class="title">
    <h1>Builds</h1>
    <span class="sub" id="sub">Worktrees, and the agents working and building in them</span>
  </div>
  <div class="controls">
    <div class="switch" role="group" aria-label="View">
      <button type="button" data-view="repos" aria-pressed="true">By repo</button>
      <button type="button" data-view="lanes" aria-pressed="false">Lanes</button>
    </div>
    <div class="disk" id="disk"><span class="label">Disk</span><span class="floors">reading</span></div>
    <span class="updated" id="updated"></span>
  </div>
</header>
<div id="errors"></div>
<main id="view"><div class="loading">Reading the checkouts</div></main>
</div>
<script>
"use strict";
const SUBTITLE = "Worktrees, and the agents working and building in them";
// The states the server derives for each side worktree (worktree_state in
// dashboard.py), in the order the cards sort.
const STATES = {
  "waiting-on-you": {label: "Waiting on you", cls: "s-you"},
  "ready-to-land": {label: "Ready to land", cls: "s-ready"},
  "winding-down": {label: "Winding down", cls: "s-winding"},
  "working": {label: "Working", cls: "s-working"},
  "landing": {label: "Landing", cls: "s-free"},
  "free": {label: "Free", cls: "s-free"},
};
const ORDER = Object.keys(STATES);
const LANES = [
  {title: "Waiting on you", hint: "A question only you can answer, or a step only you can do.", dot: "--dot-you", states: ["waiting-on-you"]},
  {title: "Ready to land", hint: "Commits on the branch whose change main doesn’t have yet.", dot: "--dot-ready", states: ["ready-to-land"]},
  {title: "Working", hint: "Claimed, with no commit waiting and nothing asked.", dot: "--dot-working", states: ["winding-down", "working", "landing"]},
  {title: "Free", hint: "Unclaimed worktrees, ready for the next agent.", dot: "--dot-free", states: ["free"]},
];
const VIEW_KEY = "builds-dashboard-view";
let state = null, folders = null, stateError = "", foldersError = "";
let view = "repos";
try {
  const saved = localStorage.getItem(VIEW_KEY);
  if (saved === "repos" || saved === "lanes") view = saved;
} catch (e) { /* no storage: the default view */ }

function h(tag, attrs, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) el.setAttribute(k, v);
  for (const kid of kids.flat()) {
    if (kid === null || kid === undefined || kid === false) continue;
    el.append(typeof kid === "string" ? document.createTextNode(kid) : kid);
  }
  return el;
}
function size(kib) {
  if (kib === null || kib === undefined) return "-";
  return kib >= 1048576 ? (kib / 1048576).toFixed(1) + "G" : Math.floor(kib / 1024) + "M";
}
function wholeSize(kib) { return size(kib).replace(".0G", "G"); }
function ago(epoch) {
  const s = Math.max(0, Date.now() / 1000 - epoch);
  if (s < 60) return Math.round(s) + "s";
  if (s < 3600) return Math.round(s / 60) + "m";
  if (s < 172800) return Math.round(s / 3600) + "h";
  return Math.round(s / 86400) + "d";
}
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
function hhmm(d) { return String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0"); }
// A local time as the clock today, "yesterday 22:52" or "Nov 3".
function when(d) {
  const today = new Date(); today.setHours(0, 0, 0, 0);
  const day = new Date(d); day.setHours(0, 0, 0, 0);
  const days = Math.round((today - day) / 86400000);
  if (days <= 0) return hhmm(d);
  if (days === 1) return "yesterday " + hhmm(d);
  return MONTHS[d.getMonth()] + " " + d.getDate();
}
// A claim's date is local time, "YYYY-MM-DD HH:MM".
function since(text) {
  const t = Date.parse(text.replace(" ", "T"));
  return isNaN(t) ? text : when(new Date(t));
}
function plural(n, one, many) { return n + " " + (n === 1 ? one : many); }
function tilde(path) { return path.replace(/^\/Users\/[^/]+/, "~"); }
function sideNumber(name) { const m = /(\d+)$/.exec(name); return m ? Number(m[1]) : 0; }

// What the folder measurement says about a checkout's build output.
function built(c) {
  if (!folders) return null;
  for (const r of folders.repos) for (const m of r.checkouts) if (m.path === c.path) return facts(m);
  return null;
}
function facts(m) {
  const out = {total: m.total_kib, newest: 0, chips: [], over: [], links: []};
  for (const f of (m.folders || [])) {
    if (!f.exists) continue;
    if (f.last_write > out.newest) out.newest = f.last_write;
    const over = f.kib > f.cap_kib;
    if (over) out.over.push(f.path + " " + size(f.kib) + " over its " + size(f.cap_kib) + " cap");
    if (f.link !== "linked") out.links.push(f.path + (f.link === "elsewhere" ? " links elsewhere" : " not linked"));
    // A folder under 1M shows as 0M: no build output worth a chip.
    if (f.kib >= 1024) out.chips.push({path: f.path, kib: f.kib, over});
  }
  out.chips.sort((a, b) => b.kib - a.kib);
  return out;
}

function pill(text, cls) { return h("span", {class: "pill " + cls}, text); }
function stateOf(c) { return STATES[c.state] || {label: c.state, cls: "s-free"}; }
function lockOf(repo) {
  const lock = repo.lock;
  return {held: !!(lock && lock.holders.length), waiting: lock ? lock.waiting : 0, holders: lock ? lock.holders : []};
}
function commands(lock) {
  return lock.holders.map(x => h("div", {class: "command"}, (x.running.length ? x.running : [x.command]).join("\n")));
}
function detachedAt(c) {
  if (c.branch) return c.branch;
  return c.git && c.git.head ? "detached at " + c.git.head.sha : "detached";
}

// Each repo's pinned checkouts (main, then the App build), its other live
// side worktrees, and the build roots whose checkout is gone.
function split(repo) {
  const main = repo.checkouts.find(c => c.live && c.main);
  const app = repo.checkouts.find(c => c.live && c.app_checkout);
  const sides = repo.checkouts.filter(c => c.live && !c.main && !c.app_checkout);
  sides.sort((a, b) => ORDER.indexOf(a.state) - ORDER.indexOf(b.state) || sideNumber(a.name) - sideNumber(b.name));
  return {main, app, sides, gone: repo.checkouts.filter(c => !c.live)};
}
function counts(cs) {
  const n = {};
  for (const c of cs) n[c.state] = (n[c.state] || 0) + 1;
  return n;
}

function totalBlock(b) {
  if (!b) return h("div", {class: "total"}, h("span", {class: "of"}, "measuring…"));
  return h("div", {class: "total"}, h("span", {class: "big"}, size(b.total)), h("span", {class: "of"}, "of build output"));
}
function chips(b) {
  if (!b) return null;
  const kids = b.chips.map(f => h("span", {class: "chip" + (f.over ? " over" : "")}, f.path + " ",
    h("span", {class: "mono"}, size(f.kib) + (f.over ? " over cap" : ""))));
  const problems = b.links.map(p => h("span", {class: "chip over"}, p));
  return kids.length || problems.length ? h("div", {class: "chips"}, kids, problems) : null;
}
function mainLine(repo) {
  return "Your checkout. No agent works or builds here" + (repo.app_checkout ? "; you install from it." : ".");
}
function appLine(lock, b) {
  if (lock.held) {
    return "An agent is building here under the app lock" +
      (lock.waiting ? "; " + plural(lock.waiting, "more waits", "more wait") + " for it." : ".");
  }
  return "Where agents build the apps, one at a time under the app lock." +
    (b && b.newest ? " Last build " + when(new Date(b.newest * 1000)) + "." : "");
}

function pinnedMain(repo, c) {
  const b = built(c);
  return h("article", {class: "pinned"},
    h("div", {class: "row1"},
      h("div", {class: "ident"}, h("span", {class: "name"}, "main"),
        h("span", {class: "where"}, tilde(c.path) + " · " + (c.branch || "detached"))),
      pill("Yours", "s-free")),
    h("span", {class: "line"}, mainLine(repo)),
    totalBlock(b), chips(b));
}
function pinnedApp(repo, c) {
  const b = built(c), lock = lockOf(repo);
  return h("article", {class: "pinned"},
    h("div", {class: "row1"},
      h("div", {class: "ident"}, h("span", {class: "name"}, "App build"),
        h("span", {class: "where"}, c.name + " · " + detachedAt(c))),
      lock.held ? pill("Lock held", "s-working") : pill("Lock free", "s-free")),
    h("span", {class: "line"}, appLine(lock, b)),
    lock.held ? commands(lock) : null,
    c.claim ? h("div", {class: "who-block"}, h("span", {class: "who"}, c.claim.who),
      c.claim.note ? h("span", {class: "note"}, c.claim.note) : null) : null,
    totalBlock(b), chips(b));
}

function waitingBox(c, max) {
  const ahead = c.git && !c.git.error ? c.git.ahead : [];
  if (!ahead.length) return null;
  return h("div", {class: "waiting"},
    h("span", {class: "label"}, plural(ahead.length, "commit", "commits") + " waiting to land"),
    ahead.slice(0, max).map(x => h("div", {class: "commit"}, h("span", {class: "sha"}, x.sha), h("span", {class: "subject"}, x.subject))),
    ahead.length > max ? h("span", {class: "more"}, "and " + (ahead.length - max) + " more") : null);
}
function buildFact(b) {
  if (!b) return h("span", {}, "build size measuring…");
  if (b.over.length) return h("span", {class: "bad", title: b.over.join("; ")}, "build " + size(b.total) + " over cap");
  return h("span", {}, "build " + size(b.total));
}
function card(c) {
  const st = stateOf(c), g = c.git, b = built(c);
  const foot = [];
  if (g && g.head) foot.push(h("span", {}, "last commit " + ago(g.head.time) + " ago"));
  if (g && !g.error) foot.push(g.uncommitted ? h("span", {}, plural(g.uncommitted, "file", "files") + " changed") : h("span", {}, "clean"));
  foot.push(buildFact(b));
  if (b) for (const p of b.links) foot.push(h("span", {class: "bad"}, p));
  if (c.claim) foot.push(h("span", {}, "since " + since(c.claim.since)));
  return h("article", {class: "card"},
    h("div", {class: "row1"},
      h("div", {class: "ident"}, h("span", {class: "name"}, c.name), h("span", {class: "where", title: detachedAt(c)}, detachedAt(c))),
      pill(st.label, st.cls)),
    h("div", {class: "who-block"},
      h("span", {class: "who"}, c.claim ? c.claim.who : "Unclaimed"),
      c.claim && c.claim.note ? h("span", {class: "note"}, c.claim.note) : null),
    g && g.error ? h("div", {class: "error"}, g.error) : null,
    waitingBox(c, 10),
    h("div", {class: "foot"}, foot));
}

function summary(parts) { return parts.filter(Boolean).join(" · "); }
function stateCounts(cs) {
  const n = counts(cs);
  return ORDER.filter(s => n[s]).map(s => n[s] + " " + STATES[s].label.toLowerCase());
}
function goneLine(gone) {
  return gone.length ? plural(gone.length, "build root whose checkout is gone", "build roots whose checkouts are gone") +
    " (" + gone.map(c => c.name).join(", ") + "; builds prune deletes them)" : null;
}

function reposView() {
  return state.repos.map(repo => {
    const {main, app, sides, gone} = split(repo);
    const pinned = [main ? pinnedMain(repo, main) : null, app ? pinnedApp(repo, app) : null].filter(Boolean);
    return h("section", {class: "repo"},
      h("div", {class: "repo-head"}, h("h2", {}, repo.name),
        h("span", {}, summary([...stateCounts(sides), goneLine(gone)]) || "no side worktrees")),
      pinned.length ? h("div", {class: "pinned-grid"}, pinned) : null,
      sides.length ? h("div", {class: "cards"}, sides.map(card)) : null);
  });
}

function compactMain(repo, c) {
  const b = built(c);
  return h("article", {class: "pinned compact"},
    h("div", {class: "row1"}, h("span", {class: "name"}, repo.name + " main"), h("span", {class: "big"}, b ? size(b.total) : "measuring…")),
    h("span", {class: "line"}, "Yours. No agent works or builds here."));
}
function compactApp(repo, c) {
  const b = built(c), lock = lockOf(repo);
  const line = lock.held
    ? "Lock held: an agent is building here" + (lock.waiting ? ", " + lock.waiting + " waiting." : ".")
    : "Lock free. Agents build the apps here, one at a time.";
  return h("article", {class: "pinned compact"},
    h("div", {class: "row1"}, h("span", {class: "name"}, repo.name + " App build · " + c.name), h("span", {class: "big"}, b ? size(b.total) : "measuring…")),
    h("span", {class: "line"}, line),
    lock.held ? commands(lock) : null);
}
function laneCard(repo, c) {
  const g = c.git, b = built(c);
  const tag = c.state === "winding-down" || c.state === "landing" ? pill(stateOf(c).label, stateOf(c).cls) : null;
  let note = c.claim && c.claim.note ? c.claim.note : null;
  if (!c.claim) note = summary([detachedAt(c), b ? "build " + size(b.total) : null]);
  return h("article", {class: "lcard"},
    h("div", {class: "row1"}, h("span", {class: "name"}, repo.name + " " + c.name),
      h("span", {class: "age"}, g && g.head ? ago(g.head.time) : "")),
    h("span", {class: "who"}, c.claim ? c.claim.who : "Unclaimed"),
    tag,
    note ? h("span", {class: "note"}, note) : null,
    g && g.error ? h("div", {class: "error"}, g.error) : null,
    g && !g.error && g.ahead.length ? h("div", {class: "commits"},
      g.ahead.slice(0, 6).map(x => h("div", {class: "commit"}, h("span", {class: "sha"}, x.sha), h("span", {class: "subject"}, x.subject))),
      g.ahead.length > 6 ? h("span", {class: "more"}, "and " + (g.ahead.length - 6) + " more") : null) : null);
}
function lanesView() {
  const pinned = [], all = [], gone = [];
  for (const repo of state.repos) {
    const s = split(repo);
    if (s.main) pinned.push(compactMain(repo, s.main));
    if (s.app) pinned.push(compactApp(repo, s.app));
    for (const c of s.sides) all.push([repo, c]);
    for (const c of s.gone) gone.push(c);
  }
  const lanes = LANES.map(lane => {
    const cards = all.filter(([, c]) => lane.states.includes(c.state));
    cards.sort((a, b) => lane.states.indexOf(a[1].state) - lane.states.indexOf(b[1].state));
    return h("section", {class: "lane"},
      h("div", {class: "lane-head"},
        h("div", {}, h("span", {class: "dot", style: "background: var(" + lane.dot + ")"}), h("h2", {}, lane.title)),
        h("span", {class: "count"}, String(cards.length))),
      h("span", {class: "hint"}, lane.hint),
      cards.length ? cards.map(([repo, c]) => laneCard(repo, c)) : h("span", {class: "none"}, "None."));
  });
  return [h("div", {class: "pinned-row"}, pinned),
    gone.length ? h("div", {class: "loading"}, goneLine(gone)) : null,
    h("div", {class: "lanes"}, lanes)];
}

function disk(v) {
  const gib = k => k / 1048576;
  const scale = Math.max(gib(v.floor_kib) * 2, gib(v.free_kib) * 1.25);
  const pct = x => Math.min(100, 100 * x / scale) + "%";
  let color = "var(--ok)", note = h("span", {class: "floors"}, "warns " + wholeSize(v.floor_kib) + " · stops " + wholeSize(v.hard_floor_kib));
  if (v.free_kib < v.hard_floor_kib) { color = "var(--bad)"; note = h("span", {class: "bad floors"}, "under " + wholeSize(v.hard_floor_kib) + ": builds stop"); }
  else if (v.free_kib < v.floor_kib) { color = "var(--warn)"; note = h("span", {class: "warn floors"}, "under " + wholeSize(v.floor_kib) + ": builds warn · stop at " + wholeSize(v.hard_floor_kib)); }
  return [h("span", {class: "label"}, "Disk"),
    h("div", {class: "bar", role: "img", "aria-label": size(v.free_kib) + " free; builds warn under " + wholeSize(v.floor_kib) + " and stop under " + wholeSize(v.hard_floor_kib)},
      h("span", {class: "fill", style: "width: " + pct(gib(v.free_kib)) + "; background: " + color}),
      h("span", {class: "mark", style: "left: " + pct(gib(v.hard_floor_kib))}),
      h("span", {class: "mark", style: "left: " + pct(gib(v.floor_kib))})),
    h("span", {class: "free"}, size(v.free_kib) + " free"),
    note];
}

// The state's age and the age of the folder sizes next to it: the server
// measures them every 5 minutes while a page is open.
function tick() {
  const el = document.getElementById("updated");
  const parts = [];
  if (state) parts.push("updated " + ago(state.generated_at) + " ago");
  parts.push(folders ? "sizes from " + ago(folders.generated_at) + " ago" : "measuring sizes…");
  el.textContent = parts.join(" · ");
}

function render() {
  for (const b of document.querySelectorAll(".switch button")) b.setAttribute("aria-pressed", String(b.dataset.view === view));
  document.getElementById("errors").replaceChildren(...[stateError, foldersError].filter(Boolean).map(e => h("div", {class: "error"}, e)));
  tick();
  if (!state) return;
  const sides = state.repos.flatMap(r => split(r).sides);
  const n = counts(sides);
  const working = (n["working"] || 0) + (n["winding-down"] || 0);
  document.getElementById("sub").textContent = summary([SUBTITLE,
    working ? working + " working" : null,
    n["ready-to-land"] ? n["ready-to-land"] + " ready to land" : null,
    n["waiting-on-you"] ? n["waiting-on-you"] + " waiting on you" : null]);
  document.getElementById("disk").replaceChildren(...disk(state.volume).filter(Boolean));
  document.getElementById("view").replaceChildren(...(view === "lanes" ? lanesView() : reposView()).filter(Boolean));
}

for (const b of document.querySelectorAll(".switch button")) {
  b.addEventListener("click", () => {
    view = b.dataset.view;
    try { localStorage.setItem(VIEW_KEY, view); } catch (e) { /* kept for this page only */ }
    render();
  });
}

async function fetchJson(path) {
  const response = await fetch(path, {cache: "no-store"});
  const body = await response.json();
  if (!response.ok) throw new Error(body.error || response.statusText);
  return body;
}
// Both polls run only while the tab is visible. Each start() begins a new
// epoch, so an answer that lands after the tab was hidden, or after a newer
// start, schedules nothing.
let epoch = 0, stateTimer = null, foldersTimer = null;
async function pollState(mine) {
  try { state = await fetchJson("/api/state"); stateError = ""; }
  catch (e) { stateError = "state: " + e.message; }
  render();
  if (mine === epoch) stateTimer = setTimeout(() => pollState(mine), 4000);
}
async function pollFolders(mine) {
  let measuring = false;
  try {
    const answer = await fetchJson("/api/folders");
    measuring = !!answer.measuring;
    folders = measuring ? null : answer;
    foldersError = answer.error ? "build folders: " + answer.error : "";
  } catch (e) { foldersError = "build folders: " + e.message; }
  render();
  // Until the first measurement ends, ask again soon.
  if (mine === epoch) foldersTimer = setTimeout(() => pollFolders(mine), measuring ? 3000 : 60000);
}
function stop() {
  epoch++;
  clearTimeout(stateTimer);
  clearTimeout(foldersTimer);
}
function start() {
  stop();
  pollState(epoch);
  pollFolders(epoch);
}
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") start(); else stop();
});
render();
setInterval(tick, 1000);
if (document.visibilityState === "visible") start();
</script>
</body>
</html>
"""

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

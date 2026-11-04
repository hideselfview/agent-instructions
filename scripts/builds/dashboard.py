#!/usr/bin/env python3
"""A page in the browser showing what every checkout `builds` knows is doing.

    dashboard.py <builds> <port>   serve it on 127.0.0.1:<port> until killed

<builds> is the `builds` script, which this asks for everything it knows:
`status --json --no-folders` for the checkouts and their claims, and
`status --json` for their build folders' sizes and last writes. Git is asked
only about side worktrees, read-only and with --no-optional-locks: HEAD, the
commits on HEAD that the local origin/main ref doesn't have, and the count of
uncommitted files. Main checkouts are the person's; for them the page shows
only what `builds status` does. Nothing is fetched.

Every request reads fresh, and the server keeps no state between requests:

    /                the page; it asks /api/state every few seconds, and
                     /api/folders a minute after each answer, since measuring
                     the folders takes most of half a minute
    /api/state       the volume's free space and floors, each repo's app.lock
                     and its holder, and each checkout with its claim and git
    /api/folders     each checkout's build folders, with sizes

A request whose Host isn't this server's own address is refused, so a page
from elsewhere that resolves its name to 127.0.0.1 can't read these.
"""

import concurrent.futures
import http.server
import json
import os
import subprocess
import sys
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
    worktree at <top>; on a failure, what git said."""
    try:
        head = commits(top, "-1", "HEAD")
        return {
            "head": head[0] if head else None,
            "ahead": commits(top, "origin/main..HEAD"),
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
            claim = checkout.get("claim")
            checkout["landing"] = bool(claim and claim["who"].endswith("(landing)"))
    status["generated_at"] = int(time.time())
    status["took_seconds"] = round(time.time() - started, 1)
    return status


def folders(builds):
    started = time.time()
    status = builds_status(builds, folders=True)
    status["generated_at"] = int(time.time())
    status["took_seconds"] = round(time.time() - started, 1)
    return status


def handler(builds, port):
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
            routes = {"/api/state": state, "/api/folders": folders}
            path = self.path.split("?", 1)[0]
            if path == "/":
                self.send(200, "text/html; charset=utf-8", PAGE)
            elif path in routes:
                try:
                    body = json.dumps(routes[path](builds))
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
<style>
:root {
  --bg: #f6f6f4; --card: #ffffff; --text: #1d1d1b; --muted: #6b6b66;
  --line: #deded8; --accent: #2457c5; --warn: #9a5b00; --warn-bg: #fff3dc;
  --bad: #b3261e; --bad-bg: #fde7e5; --ok: #1e7a3c; --chip: #ecece6;
  --mono: ui-monospace, SFMono-Regular, Menlo, monospace;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #161615; --card: #20201f; --text: #e8e8e4; --muted: #9a9a94;
    --line: #34342f; --accent: #8fb0ff; --warn: #f0b660; --warn-bg: #3a2c12;
    --bad: #ff8a80; --bad-bg: #3d1a17; --ok: #7fd49a; --chip: #2e2e2b;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; background: var(--bg); color: var(--text);
  font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}
main { padding: 16px; }
h1 { font-size: 18px; margin: 0 0 4px; }
h2 { font-size: 16px; margin: 24px 0 8px; }
.meta { color: var(--muted); font-size: 12px; }
.top { display: grid; gap: 8px; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); margin: 12px 0; }
.panel { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 10px 12px; min-width: 0; }
.bar { height: 8px; background: var(--chip); border-radius: 4px; position: relative; margin: 6px 0 2px; overflow: hidden; }
.bar > span { position: absolute; left: 0; top: 0; bottom: 0; background: var(--ok); }
.bar > i { position: absolute; top: 0; bottom: 0; width: 2px; background: var(--muted); }
.grid { display: grid; gap: 10px; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); }
.card { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 10px 12px; min-width: 0; }
.card.claimed { border-left: 3px solid var(--accent); }
.head { display: flex; flex-wrap: wrap; gap: 6px; align-items: baseline; }
.name { font-weight: 600; font-size: 15px; }
.chip { font-size: 11px; padding: 1px 6px; border-radius: 9px; background: var(--chip); color: var(--muted); }
.chip.app { color: var(--accent); }
.chip.landing { color: var(--ok); }
.chip.gone { color: var(--bad); }
.path, .mono { font-family: var(--mono); font-size: 12px; overflow-wrap: anywhere; }
.path { color: var(--muted); }
.row { margin-top: 4px; overflow-wrap: anywhere; }
.label { color: var(--muted); }
.note { margin-top: 6px; padding: 4px 8px; border-radius: 6px; background: var(--warn-bg); color: var(--warn); }
ul { margin: 2px 0 0; padding-left: 18px; }
li { overflow-wrap: anywhere; }
.warn { color: var(--warn); }
.bad { color: var(--bad); }
.error { background: var(--bad-bg); color: var(--bad); border-radius: 6px; padding: 6px 10px; margin: 8px 0; white-space: pre-wrap; }
</style>
</head>
<body>
<main>
<h1>Builds</h1>
<div class="meta" id="meta">loading</div>
<div id="error"></div>
<div class="top" id="top"></div>
<div id="repos"></div>
</main>
<script>
"use strict";
let state = null, folders = null, stateError = "", foldersError = "";

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
function ago(epoch) {
  if (!epoch) return "-";
  const s = Math.max(0, Date.now() / 1000 - epoch);
  if (s < 90) return Math.round(s) + "s ago";
  if (s < 5400) return Math.round(s / 60) + "m ago";
  if (s < 172800) return (s / 3600).toFixed(1) + "h ago";
  return Math.round(s / 86400) + "d ago";
}
function clock(epoch) { return new Date(epoch * 1000).toLocaleTimeString(); }
// A claim's date is local time, "YYYY-MM-DD HH:MM".
function since(text) {
  const t = Date.parse(text.replace(" ", "T"));
  return isNaN(t) ? text : text + " (" + ago(t / 1000) + ")";
}

function volumePanel(v) {
  const gib = k => k / 1048576;
  const scale = Math.max(gib(v.floor_kib) * 2, gib(v.free_kib) * 1.2);
  const pct = x => Math.min(100, 100 * x / scale) + "%";
  let cls = "", word = "above the floors";
  if (v.free_kib < v.hard_floor_kib) { cls = "bad"; word = "under the hard floor: builds stop"; }
  else if (v.free_kib < v.floor_kib) { cls = "warn"; word = "under the floor: builds warn"; }
  const bar = h("div", {class: "bar"},
    h("span", {style: "width:" + pct(gib(v.free_kib)) + (cls ? ";background:var(--" + cls + ")" : "")}),
    h("i", {style: "left:" + pct(gib(v.hard_floor_kib)), title: "hard floor"}),
    h("i", {style: "left:" + pct(gib(v.floor_kib)), title: "floor"}));
  return h("div", {class: "panel"},
    h("div", {}, h("b", {}, size(v.free_kib) + " free"), " of " + size(v.size_kib) + " ",
      h("span", {class: cls}, word)),
    bar,
    h("div", {class: "meta"}, "floor " + size(v.floor_kib) + ", hard floor " + size(v.hard_floor_kib)));
}

function lockPanel(repo) {
  const lock = repo.lock;
  const kids = [h("div", {}, h("b", {}, repo.name + " app.lock"),
    repo.app_checkout ? " (app checkout " + repo.app_checkout + ")" : "")];
  if (!lock || lock.holders.length === 0) kids.push(h("div", {class: "meta"}, "free"));
  for (const holder of (lock ? lock.holders : [])) {
    kids.push(h("div", {class: "row"}, h("span", {class: "warn"}, "held"), " by pid " + holder.pid));
    for (const c of (holder.running.length ? holder.running : [holder.command]))
      kids.push(h("div", {class: "mono"}, c));
  }
  if (lock && lock.waiting) kids.push(h("div", {class: "row"}, lock.waiting + " waiting"));
  return h("div", {class: "panel"}, kids);
}

function card(c, built) {
  const claimed = !!c.claim;
  const chips = [];
  if (c.main) chips.push(h("span", {class: "chip"}, "main checkout"));
  if (c.app_checkout) chips.push(h("span", {class: "chip app"}, "app build"));
  if (c.landing) chips.push(h("span", {class: "chip landing"}, "landing"));
  if (!c.live) chips.push(h("span", {class: "chip gone"}, "checkout gone"));
  const kids = [
    h("div", {class: "head"}, h("span", {class: "name"}, c.name), chips),
    h("div", {class: "path"}, c.path),
  ];
  if (c.live) kids.push(h("div", {class: "row"}, h("span", {class: "label"}, "branch "),
    h("span", {class: "mono"}, c.branch || "detached")));
  if (claimed) {
    kids.push(h("div", {class: "row"}, h("span", {class: "label"}, "claimed by "), h("b", {}, c.claim.who),
      " since " + since(c.claim.since)));
    if (c.claim.note) kids.push(h("div", {class: "note"}, c.claim.note));
  }
  const g = c.git;
  if (g && g.error) kids.push(h("div", {class: "error"}, g.error));
  if (g && g.head) kids.push(h("div", {class: "row"}, h("span", {class: "label"}, "HEAD "),
    h("span", {class: "mono"}, g.head.sha), " " + g.head.subject + " ",
    h("span", {class: "meta"}, ago(g.head.time))));
  if (g && !g.error) {
    if (g.ahead.length) {
      kids.push(h("div", {class: "row"}, h("span", {class: "label"},
        g.ahead.length + " commit" + (g.ahead.length === 1 ? "" : "s") + " not on origin/main")));
      kids.push(h("ul", {}, g.ahead.slice(0, 30).map(x =>
        h("li", {}, h("span", {class: "mono"}, x.sha), " " + x.subject))));
      if (g.ahead.length > 30) kids.push(h("div", {class: "meta"}, "and " + (g.ahead.length - 30) + " more"));
    } else {
      kids.push(h("div", {class: "row meta"}, "nothing waiting to land"));
    }
    kids.push(h("div", {class: "row"}, h("span", {class: "label"}, "uncommitted "),
      h("span", {class: g.uncommitted ? "warn" : ""}, g.uncommitted + " file" + (g.uncommitted === 1 ? "" : "s"))));
  }
  if (built) {
    let newest = 0;
    const problems = [];
    for (const f of (built.folders || [])) {
      if (!f.exists) continue;
      if (f.last_write > newest) newest = f.last_write;
      if (f.kib > f.cap_kib) problems.push(f.path + " over its cap (" + size(f.kib) + " / " + size(f.cap_kib) + ")");
      if (f.link !== "linked") problems.push(f.path + " " + (f.link === "elsewhere" ? "links elsewhere" : "not linked"));
    }
    kids.push(h("div", {class: "row"}, h("span", {class: "label"}, "builds "), size(built.total_kib),
      built.folders ? ", newest write " + ago(newest) : ""));
    for (const p of problems) kids.push(h("div", {class: "row bad"}, p));
  } else {
    kids.push(h("div", {class: "row meta"}, "builds: measuring"));
  }
  return h("div", {class: "card" + (claimed ? " claimed" : "")}, kids);
}

function render() {
  const meta = [];
  if (state) meta.push("updated " + clock(state.generated_at) + " (" + state.took_seconds + "s)");
  if (folders) meta.push("build folders measured " + clock(folders.generated_at) + " (" + folders.took_seconds + "s)");
  else meta.push("measuring build folders");
  document.getElementById("meta").textContent = meta.join(" · ");
  const errors = document.getElementById("error");
  errors.replaceChildren(...[stateError, foldersError].filter(Boolean).map(e => h("div", {class: "error"}, e)));
  if (!state) return;
  document.getElementById("top").replaceChildren(volumePanel(state.volume),
    ...state.repos.filter(r => r.app_lock).map(lockPanel));
  const measured = {};
  for (const r of (folders ? folders.repos : []))
    for (const c of r.checkouts) measured[r.name + "/" + c.name] = c;
  document.getElementById("repos").replaceChildren(...state.repos.map(r => {
    const rank = c => c.main ? 0 : c.app_checkout ? 1 : c.claim ? 2 : 3;
    const order = r.checkouts.map((c, i) => [rank(c), i, c]).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    const claimed = r.checkouts.filter(c => c.claim).length;
    return h("section", {},
      h("h2", {}, r.name, h("span", {class: "meta"}, "  " + r.checkouts.length + " checkouts, " + claimed + " claimed")),
      h("div", {class: "grid"}, order.map(([, , c]) => card(c, measured[r.name + "/" + c.name]))));
  }));
}

async function fetchJson(path) {
  const response = await fetch(path, {cache: "no-store"});
  const body = await response.json();
  if (!response.ok) throw new Error(body.error || response.statusText);
  return body;
}
async function pollState() {
  try { state = await fetchJson("/api/state"); stateError = ""; }
  catch (e) { stateError = "state: " + e.message; }
  render();
  setTimeout(pollState, 4000);
}
async function pollFolders() {
  try { folders = await fetchJson("/api/folders"); foldersError = ""; }
  catch (e) { foldersError = "build folders: " + e.message; }
  render();
  setTimeout(pollFolders, 60000);
}
pollState();
pollFolders();
</script>
</body>
</html>
"""

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

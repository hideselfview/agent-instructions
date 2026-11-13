# Coordinating background agents

How one coordinating session runs several background agents across repos
(elaborates "Agent workflow"). The coordinator plans, dispatches, reviews and
lands; agents implement. The project file (e.g. `projects/bae.md`) names the
project's lists, worktrees and landing worktree.

## The lists

The decision stack, the work queue, the decision log and the design changes live
in the `builds` store (`builds help lists`); the project file says which lists a
project has. A command refuses a step that lacks what it needs, so the rules
here are only those it can't check.

- **The decision stack**: what waits on the person, asked one at a time from the
  top. A plan's open question goes here.
- **The work queue**: only what agents can do. `builds next` lists what is
  ready, in dispatch order.
- **The decision log**: each answer, and each standing choice about how work
  runs, with why.

## Plan, then brief, then implement

1. **Plan** (Opus, the coordinator): for work whose shape is still open, a plan
   in `plans/<name>.md`; its open questions go on the decision stack.
2. **Brief** (the coordinator): each queued item gets a brief that states the
   goal, the shape, the facts gathered so far (measurements, logs, leads), what
   must hold, and the checks. It leaves the executor no design decision, but it
   doesn't write the change file by file: the executor reads the code. The
   template is `implementation-briefs-template.md`.
3. **Execute** (Sonnet, as of 2026-11): every dispatched agent ("Plan with Opus,
   execute with Sonnet").
4. **Review** every commit before landing ("Review every executor's work").

## What an implementing agent does

1. **Read the real code paths** its brief names, not summaries; a fork the brief
   didn't settle goes back to the coordinator, not into a guess.
2. **Branch from `origin/main`** in its claimed worktree.
3. **Implement** to the brief; a bug fix starts with its failing test.
4. **One manual review** of its own diff against the brief and the matching rule
   bodies.
5. **Local checks: only what the change touches**: the hook, builds of touched
   platforms (app builds in the app checkouts, a mac and an android one, each
   under its lane's lock), tests covering the changed code. Never a whole app
   test suite (it launches the app on the person's screen) or the full CI
   script: CI runs those. A project with no pre-commit hook gets one.
6. **Commit and report** the SHA, base and checks; the coordinator lands it.

Every agent the coordinator starts shares its session's scratchpad directory, so
each one keeps its scratch files (commit messages, receipts, probes) in a folder
of its own under it, named for its branch. One agent's commit once went out with
another's message, written to the same file name.

## Dispatch

- Who executes: the coordinator settles the shape and writes the brief; Sonnet
  executes it. A dispatched agent runs Opus only for a design too large for the
  coordinator to hold, and the brief says why: what the coordinator read, and
  what stays unknown. That agent writes a plan; Sonnet executes it.
- One task per agent (a commit or a short chain); later items go to the same
  agent while its context is small, to a successor from its handoff once it
  passes about 550k tokens.
- Give work to the agent that already holds its context: a finding goes back to
  the agent that reported it, and an item in an area goes to the agent that just
  finished work there, resumed with SendMessage, before any fresh agent. A fresh
  agent rereads the same files and the same commits first. Start a new one only
  when no finished agent under the handoff size knows the area, or when the item
  needs the other model.
- Each brief to an agent names its worktree, its items, the rules it must read,
  the project's hard limits (main checkouts, keychain, devices, cargo through
  the wrapper, no `git add -A`/stash, and no `--no-verify`), the mode, the
  model's commit trailer, and "report each commit; don't push main". A
  quick-mode brief names the one test that proves the change and the clippy
  command with the feature sets the hook builds; it never says "the tests of
  what you touched". An agent given that line on 2026-11-06 ran eight crates'
  whole suites for 77 minutes, as told, then failed the hook on a feature set
  its check had not built.
- Take items from `builds next`. Claim the worktree (`builds claim`), then mark
  the items running with `builds item take N --worktree W --branch B --brief P`;
  the claim names the agent. After a compact, `builds item list --running`
  prints each running item's agent, worktree, branch and brief. A new agent
  continues a handoff with `builds item handoff N --brief P`; an agent that
  stops, with `builds item stop N --why WHY`. `builds release` refuses a
  worktree an item still runs in. Keep the worktree's note current
  (`builds note`) on every dispatch, handoff and landing, and release it when
  the work lands. The dashboard (`builds dashboard`) shows the same, with the
  lists in its sidebar.
- Read-only agents (planners, audits) read through `git show`/`git grep` on
  `origin/main`, never a checkout.
- Disk is a queue limit, never a question for the person. Under the builds
  tool's floor, start nothing new; work waits in the queue. As agents finish,
  release their worktrees, let `builds prune --idle` delete their build folders,
  and start the next work. Never ask the person to clear warm checkouts to make
  room.
- Keep the queue moving: when an agent finishes, land its commits and start the
  next work in the same turn (within the mode's limits), before writing up
  anything else.

## Modes

The person picks the mode; the builds dashboard shows the current one.

- **Strict.** Agents commit through the pre-commit hook and run the tests of
  what they touched. Load is a queue limit: while `builds health` shows load
  well over the core count, start nothing new; running agents carry on, and work
  waits until enough of them finish to bring the load down.
- **Quick.** Load is no queue limit; keep 8 agents running, topping up at every
  health check and every landing. Before each commit an agent compiles what it
  touched (`cargo clippy --all-targets -- -D warnings` with the project's test
  features, and the apps' test-target compiles for Swift or Kotlin changes) and
  commits through the hook, which in quick mode runs fmt, the static checks, and
  a compile of what changed (no clippy, rustdoc or tests). Agents run only the
  one test that proves a fix. land.sh's compile of the touched crates, clippy
  included, still gates every landing, so main always builds; an agent that
  skips clippy finds its lint at landing, a round trip later. At the full-suite
  run the person sets, a fixer agent runs every suite and CI, and fixes each
  failure with the commit that caused it. A test that fails during the window is
  checked on `origin/main` first; when it fails there too and isn't the agent's,
  the agent notes it and moves on.

The mode is one machine-wide setting the pre-commit hooks of every repo read:
the coordinator switches it with `builds mode quick` or `builds mode strict`
(`builds mode` prints it; unset means strict), and a hook prints the mode it
ran. Agents never set it. Disk under the floor holds new dispatch in both modes.
Each brief names the mode, and switching modes means telling every running
agent.

## Landing

- Land from the claimed landing worktree with
  `scripts/land/land.sh <worktree> <sha>...`: it fast-forwards when the commits
  sit on origin/main, else cherry-picks them, and retries a refused push. A
  cherry-picked result is a tree no hook has seen, so before pushing it land.sh
  runs the commands in the repo's `.land-checks` (one per line, run from the
  repo root: the cheap whole-tree checks commits can break together, such as the
  file size cap) and refuses to push when one fails, leaving the worktree at
  origin/main. A fast-forward skips them. No compile or test step: that stays
  with the coordinator.
- In a project with lists,
  `land.sh <worktree> --done N|--part N [--design N] <sha>...` names the items
  the commits finish (`--done`) or advance (`--part`), and checks them before it
  fetches anything. Commits that change a design place (a board, a component, a
  spec section) also need `--design N`, the design change recorded for them,
  which the person has approved. Commits that are all design files land with
  `--design N` alone; that is how the design session lands, from the claimed
  landing worktree. The records are written after the push; when `land.sh`
  prints `LANDED, NOT RECORDED:`, run the `rerun:` command it prints (safe to
  repeat).
- After landing commits whose base is behind main and that touch shared code
  (schema, test stores, bridge types), build main
  (`check --workspace --all-targets --all-features`) before landing more; send
  agents whose base changed under them back to rebase rather than cherry-picking
  blind.
- Don't land in the landing worktree while a check runs there.
- A CI run on main that goes red or stalls is the coordinator's to run down.

## Health check

Every 5 minutes, scheduled in the coordinator's session (CronCreate, which only
a schedule can do: nothing else wakes a session), the coordinator runs
`builds health` and acts on its "needs action" list before anything else:

- **sccache finished no compiles while clients wait:** the server is stuck;
  restart it (`launchctl kickstart -k gui/$(id -u)/local.sccache-server`).
  Compiles in flight fail once and rerun.
- **Junk shells:** `builds health` lists the agent shells that wait (sleep,
  `until`/`while` loops, `caffeinate -t`), search the whole disk, copy a command
  already running in the same worktree, or sit idle; it kills nothing, so the
  coordinator stops each by PID.
- **A claimed worktree with no change and no build for an hour:** ask its agent
  what it waits on, or land and release it if the work is done. A claim kept on
  purpose with nothing running gets
  `builds note <worktree> --state held "<why, and until when>"`, which health
  doesn't flag.
- **A running item whose worktree is gone or unclaimed:** `builds health` names
  it; land it, stop it (`builds item stop`) or hand it on. A lists store that
  fails its check is named the same way; nothing repairs it.
- **Disk under the floor:** hold the queue (Dispatch).
- **An app lock held long, or with agents waiting:** each lane (mac, android)
  has its own lock and queue, shown separately; check the holder is building,
  not stuck. A step past its time limit (45 minutes, or its `limit` line in
  `.app-build`) is killed and fails the build, which passes the lock on, so a
  hung step ends by itself; a build that is slow but alive is not killed.

In quick mode each check also runs `scripts/agent-runs/check.py`, which lists
running cargo builds and tests outside quick mode (a feature set other than the
hook's, a test run with no name filter or over several crates), and tells each
one's agent to stop it.

A quiet check needs no message to the person.

The schedule runs only while there is something to check: agents of the
coordinator's running, or commits waiting to land. Once the last one lands and
nothing runs, the coordinator deletes it (CronDelete), and creates it again with
its next dispatch.

## Background shells

Notify, don't poll; one watcher with a hard timeout. `builds health` lists the
junk shells its agents left behind, and the coordinator stops them by PID.

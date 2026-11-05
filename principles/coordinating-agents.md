# Coordinating background agents

How one coordinating session runs several background agents across repos
(elaborates "Agent workflow"). The coordinator plans, dispatches, reviews and
lands; agents implement. The project file (e.g. `projects/bae.md`) names the
project's lists, worktrees and landing worktree.

## The lists

- **The decision stack** (§1 of the project's todo file): what waits on the
  person, newest on top, asked one at a time. A plan's open question goes here.
- **The work queue** (§2): only what agents can do, grouped by area. A running
  item starts with "Running: <agent>, <worktree> <branch>, brief/handoff
  <path>"; a running item sits under "Running", moved there in the same edit
  that marks it. A landed item leaves the list in the same edit as its landing.
- **The decision log**: each answer, and each standing choice about how work
  runs, with why.

## Plan, then brief, then implement

1. **Plan** (orchestrator model): for work whose shape is still open, a plan in
   `plans/<name>.md`; its open questions go on the decision stack.
2. **Brief** (orchestrator model): each queued item gets a brief in
   `plans/briefs/<area>.md`, written from the code, that leaves the implementer
   no design decision: what the code does now (path:line), the change file by
   file, the failing test, the checks, the spec sections, locales, whether a
   reset is needed, the model, and what it must follow. The template is
   `implementation-briefs-template.md`.
3. **Implement**: the implementer model (as of 2026-11, Claude Sonnet) takes
   briefed items; the orchestrator model (as of 2026-11, Claude Opus) keeps
   design, concurrency, schema, open shapes, audits and reviews ("Pick each
   agent's model by how settled the work is").
4. **Review a sample** before landing: about one implementer commit in five,
   plus every tricky one ("Sample the implementer's work").

## What an implementing agent does

1. **Read the real code paths** its brief names, not summaries; a fork the brief
   didn't settle goes back to the coordinator, not into a guess.
2. **Branch from `origin/main`** in its claimed worktree.
3. **Implement** to the brief; a bug fix starts with its failing test.
4. **One manual review** of its own diff against the brief and the matching rule
   bodies.
5. **Local checks: only what the change touches**: the hook, builds of touched
   platforms (app builds in the app checkout under its lock), tests covering the
   changed code. Never a whole app test suite (it launches the app on the
   person's screen) or the full CI script: CI runs those. A project with no
   pre-commit hook gets one.
6. **Commit and report** the SHA, base and checks; the coordinator lands it.

## Dispatch

- One task per agent (a commit or a short chain); later items go to the same
  agent while its context is small, to a successor from its handoff once it
  passes about 550k tokens.
- Each brief to an agent names its worktree, its items, the rules it must read,
  the project's hard limits (main checkouts, credentials, keychain, devices,
  cargo through the wrapper, no `git add -A`/stash/`--no-verify`), the model's
  commit trailer, and "report each commit; don't push main".
- Claim the worktree (`builds claim`), keep its note current (`builds note`) on
  every dispatch, handoff and landing, and release it when the work lands. The
  dashboard (`builds dashboard`) shows the same.
- Read-only agents (planners, audits) read through `git show`/`git grep` on
  `origin/main`, never a checkout.
- Disk is a queue limit, never a question for the person. Under the builds
  tool's floor, start nothing new; work waits in the queue. As agents finish,
  release their worktrees, let `builds prune --idle` delete their build folders,
  and start the next work. Never ask the person to clear warm checkouts to make
  room.
- Keep the queue moving: when an agent finishes, land its commits and start the
  next work in the same turn, before writing up anything else.

## Landing

- Land from the claimed landing worktree with
  `scripts/land/land.sh <worktree> <sha>...`: it fast-forwards when the commits
  sit on origin/main, else cherry-picks them, and retries a refused push.
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
- **A Claude shell older than 2 hours:** read what it runs; stop it by PID when
  it waits on something that is over or never comes.
- **A claimed worktree with no change and no build for an hour:** ask its agent
  what it waits on, or land and release it if the work is done. A claim kept on
  purpose with nothing running gets
  `builds note <worktree> --state held "<why, and until when>"`, which health
  doesn't flag.
- **Disk under the floor:** hold the queue (Dispatch).
- **An app lock held long, or with agents waiting:** check its holder is
  building, not stuck.

A quiet check needs no message to the person.

## Background shells

Notify, don't poll; one watcher with a hard timeout. A coordinator checks now
and then for shells its agents left behind (`ps` children of the session) and
stops stale ones by PID.

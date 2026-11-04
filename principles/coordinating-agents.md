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
- Watch disk before starting an agent that needs a cold build: under the builds
  tool's warning floor, wait for a slot.

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

## Background shells

Notify, don't poll; one watcher with a hard timeout. A coordinator checks now
and then for shells its agents left behind (`ps` children of the session) and
stops stale ones by PID.

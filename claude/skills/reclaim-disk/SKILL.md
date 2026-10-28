---
name: reclaim-disk
description: Free disk space by deleting regenerable build artifacts and caches in priority order (/private/tmp, then idle build folders on the build volume via `builds prune --idle` and real build dirs under ~/dev, then ~/Library/Caches). Use when the disk is full or low, a command fails with "no space left on device" (ENOSPC), or the user asks to clean up build artifacts / reclaim space. Nothing runs it on a timer.
---

# Reclaim disk space

Deletes **regenerable** build output and caches until free space climbs back
above a target. Nothing it removes is irreplaceable — every target is a build
artifact or cache that rebuilds on next use.

Runner: `~/dev/agent-instructions/scripts/reclaim-disk/reclaim.sh`
(tool-agnostic shell script — works the same whether invoked by Claude or
Codex).

## Why disks fill here

Agent/automation flows (rules-review, verify, composer-mode, MCP builds) point
`CARGO_TARGET_DIR` and Xcode `DerivedData` into `/private/tmp` and never clean
up. Each run drops a 5–14 GB build dir there; they pile to 100 GB+ and fill the
disk **invisibly** — `df /` reports the small sealed system snapshot, and a
home-dir scan never looks in `/tmp`, so the space appears to vanish for no
reason. (See also: the real free space lives on `/System/Volumes/Data`, not the
`/` that `df` shows by default.)

## Tiers (deleted in order, stopping once free ≥ target)

1. **`/private/tmp`** — throwaway scratch + the redirected build dirs above.
   Biggest win, safest. Skips three kinds of listening-socket dir that hold no
   disk weight and break live processes when removed: `claude-*` (the live
   Claude Code session scratch), `com.apple.*` (launchd's per-session socket
   dirs), and `tmux-*` (the tmux server socket — its mtime is the server's start
   time, so a long-lived server always looks idle, and unlinking the socket
   strands the running server and every pane in it beyond reach of
   `tmux attach`).
2. **Build caches** — first the build volume: `builds prune --idle <IDLE_MIN>`
   deletes every checkout's build folder
   (`/Volumes/Builds/<repo>/<checkout>/ <path>`) that is over its cap or that
   nothing wrote to within `IDLE_MIN`, and recreates it empty so the checkout's
   link stays valid (see `principles/worktree-build-priming.md`). Main's and the
   app checkout's folders stay warm unless the person asks to clear them too
   (`--include-warm`). Then real `target*/`, `.build`, `DerivedData` dirs under
   `~/dev` (repos without a `.build-dirs`; links are not matched). Deleting only
   costs a rebuild, mostly from sccache. `node_modules` is intentionally **not**
   touched (it needs a reinstall, not a rebuild).
3. **`~/Library/Caches`** + tool caches (npm / gradle / cargo / `~/.cache`). A
   cache linked onto the build volume keeps its link and loses its contents; the
   sccache cache is left alone, since its 40 GiB cap bounds it.

A checkout's build output lives on the build volume behind links
(`~/dev/bae/target -> /Volumes/Builds/bae/main/target`), so a home-dir scan
never sees it; `builds status` lists it per checkout and folder. The build
volume shares free space with the main disk (same APFS container), so freeing
either helps both. Day to day the caps in each repo's `.build-dirs` bound each
folder, and `builds setup`, which every build runs first, keeps a free-space
floor (30 GiB): under it, it deletes whole build folders across checkouts that
no build is using and nothing wrote to in 30 minutes, least recently written
first, never main's or the repo's app checkout's, and fails the build, naming
the biggest folders, if that is not enough. That failure is when this skill's
other tiers are needed.

Three guards keep it from breaking live work: it acts only when free space is
below the threshold, scans the complete candidate tree for writes within
`IDLE_MIN`, and skips build output a Cargo, Rust, Swift, or Xcode process is
working on. Checking descendants matters because compilers usually update files
below `target/` without changing `target/` itself.

"Working on" is decided by open files: a build holding anything open under the
dir or its checkout owns it (cargo keeps `<dir>/*/.cargo-lock` for the whole
build). On the build volume, `builds` keeps any folder any process holds a file
open in.

## Run it by hand

```bash
scripts/reclaim-disk/reclaim.sh            # act only if free < 40 GB
scripts/reclaim-disk/reclaim.sh --force    # run every tier regardless
scripts/reclaim-disk/reclaim.sh --dry-run  # show what would go, delete nothing
scripts/reclaim-disk/reclaim.sh --threshold 50 --target 80
```

Tunables (env or flags): `RECLAIM_THRESHOLD_GB` (default 40),
`RECLAIM_TARGET_GB` (60), `RECLAIM_IDLE_MIN` (30). Log:
`~/Library/Logs/reclaim-disk.log`.

## No automatic sweep

There is no launchd timer. The script runs only when a person or an agent
invokes it. A timer once ran it every 15 minutes; while free space stayed under
the threshold it deleted the build output of checkouts in active use and the
compiler caches behind them, so every build was a cold one. The size caps and
the free-space floor the `builds` tool applies at the start of every build are
what bound the build volume now; the floor never deletes a folder written in the
last 30 minutes, one a build holds, or main's or the app checkout's.

## When the sweep can't help

If the log says `WARN still below threshold after all tiers`, the remaining
usage is **real data**, not build junk — investigate by walking the data volume
directly (normal `du` of `~` misses system paths):

```bash
sudo du -shx /System/Volumes/Data/* 2>/dev/null | sort -rh | head
```

Common real-data hogs that need a human decision (the script never touches
these): podman/Docker VM images, Android emulators (`~/.android/avd`), iOS
simulator runtimes (`/Library/Developer/CoreSimulator`), `~/Downloads`,
`~/Torrents`.

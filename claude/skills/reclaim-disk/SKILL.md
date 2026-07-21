---
name: reclaim-disk
description: Free disk space by deleting regenerable build artifacts and caches in priority order (/private/tmp, then ~/dev build dirs, then ~/Library/Caches). Use when the disk is full or low, a command fails with "no space left on device" (ENOSPC), or the user asks to clean up build artifacts / reclaim space. Also documents the launchd agent that runs the sweep automatically every 15 minutes.
---

# Reclaim disk space

Deletes **regenerable** build output and caches until free space climbs back
above a target. Nothing it removes is irreplaceable — every target is a build
artifact or cache that rebuilds on next use.

Runner: `~/dev/agent-instructions/scripts/reclaim-disk/reclaim.sh`
(tool-agnostic shell script — works the same whether invoked by Claude, Codex,
or the launchd timer).

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
   Biggest win, safest. Skips the live `claude-*` session scratch.
2. **`~/dev` build caches** — Rust `target*/`, `.build`, `DerivedData`. Deleting
   only costs a recompile. `node_modules` is intentionally **not** touched (it
   needs a reinstall, not a rebuild).
3. **`~/Library/Caches`** + tool caches (npm / gradle / cargo / `~/.cache`).

Three guards keep it from breaking live work: it acts only when free space is
below the threshold, scans the complete candidate tree for writes within
`IDLE_MIN`, and skips build output when a Cargo, Rust, Swift, or Xcode process
owns the containing checkout. Checking descendants matters because compilers
usually update files below `target/` without changing `target/` itself.

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

## Automatic sweep (every 15 min)

`install.sh` installs a launchd agent (`com.dima.reclaim-disk`) that runs the
script every 900 s. It only deletes when free space is below the threshold, so
most runs are no-ops. Manage it:

```bash
launchctl list | grep reclaim-disk                       # is it loaded?
launchctl bootout  gui/$(id -u)/com.dima.reclaim-disk     # stop it
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.dima.reclaim-disk.plist
tail -f ~/Library/Logs/reclaim-disk.log                  # watch it work
```

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

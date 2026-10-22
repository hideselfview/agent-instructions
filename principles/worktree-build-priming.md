# Build output per checkout, and the `builds` tool

How build output is laid out on this machine for repos with several checkouts
(the main checkout plus side worktrees `side-1`, `side-2`, …): Rust, Xcode,
Gradle, dotnet and Lean output alike. Pairs with the worktree rules in
`instructions-agent.md`; this file is the build-infra layer those rules gesture
at with "whatever per-worktree setup the project runs."

## The layout

Every checkout owns one build root at a fixed path on the build volume:

```
/Volumes/Builds/<repo>/<checkout>/        checkout = main | side-N
```

The repo lists its build output paths, each with a size cap, in `.build-dirs` at
its root (`target`, `target-macos`, Xcode `.build` dirs, Gradle `build` and
`.gradle`, dotnet `obj`/`bin`, Lean `.lake`). Each listed path is a symlink in
the checkout to the same path under the checkout's root:
`~/dev/bae/target -> /Volumes/Builds/bae/main/target`.

The tool is `builds` (`~/dev/agent-instructions/scripts/builds/builds`, on PATH
as `~/.local/bin/builds`):

- `builds setup [<checkout>]` creates the links, recreates any folder behind a
  link that was deleted, deletes a folder an older layout left behind (never
  moves it: Xcode's DerivedData records its own absolute path, so a moved one
  can't find its packages), applies the caps to that checkout, and keeps the
  volume's free-space floor (below). The repo's `scripts/build-layout.sh` calls
  it, and the hooks and every build script call that first. Without the tool
  (CI) the script does nothing and the tools write into real directories.
- `builds new [<repo>] [-b <branch>] [--no-prime]` adds the next free `side-N`
  from `origin/main`, links it, and runs its own `post-checkout` (which, in bae,
  primes the macOS build unless `--no-prime`).
- `builds retire <checkout>` removes a clean side worktree and its build root.
- `builds status` lists every root: size per folder against its cap, last write,
  whether the link is in place.
- `builds prune [--dry-run] [--idle <minutes>]` applies the caps everywhere and
  deletes roots whose checkout is gone; `--idle` also deletes every folder
  nothing wrote to for that long (the reclaim-disk skill uses it).
- `builds install` puts the tool on PATH, writes the sccache config and runs the
  sccache server under launchd.

## Why one root per checkout, at a path that never changes

- **No shared target dir.** Two checkouts building into one dir serialize on
  cargo's build-dir lock (the second waited 84 s) and then recompile what the
  other changed (13 units). Separate dirs build concurrently. Don't set
  `CARGO_TARGET_DIR` in the shell profile or inline; the checkout's `target`
  link is the target dir.
- **The path is part of the sccache key.** sccache hashes every `CARGO_*`
  variable (including `CARGO_TARGET_DIR`), the working directory, `OUT_DIR` and
  the `--extern` paths. A build into a different path gets 0% hits; the same
  path deleted and rebuilt gets 100% (137 s against 332 s cold). Tools always
  write through `<checkout>/<path>`, so deleting the folder behind a link never
  costs the cache.
- **No extra `CARGO_*` variables in scripts.** `CARGO_NET_OFFLINE`,
  `CARGO_PROFILE_*` and the like split the cache the same way (0 of 15 hits).
  `RUSTC_WRAPPER=""` turns the cache off; nothing needs it on macOS (Windows CI
  sets it for the 32K command-line limit).
- **A dangling link breaks every tool** — cargo (os error 20), xcodebuild (rc
  74), Gradle ("Could not open file hash cache"), dotnet (NuGet/MSB3021), lake
  (error 17). Recreating the folder fixes all of them, which is why every entry
  point runs `builds setup` before building.
- **A build in a linked worktree must not depend on the worktree's git dir
  holding shared refs.** `.git/worktrees/<name>/` has no `refs/tags`; a build
  script that watches a missing path reruns on every build. Watch shared refs
  through `git rev-parse --git-common-dir`.

## Disk stays bounded by caps, not by sweeps

Each folder is deleted whole (renamed into `/Volumes/Builds/.trash`, removed in
the background, recreated empty) when it grows past its cap, unless a build is
using it. Hooks and build scripts in one checkout overlap, so "in use" is
decided while holding what a build holds: every `.cargo-lock` flock in the
folder, taken without waiting (cargo holds one for a whole build), then a fresh
check that no process has a file or working directory inside it, then the
rename, then the locks are released. A cargo build that starts meanwhile queues
on the lock and writes into the new folder. Caps are about twice a fresh build
of the folder; the next build refills it, mostly from sccache (a wipe and
rebuild took 137 s and 8.9 GB).

Caps bound folders, not the volume: a bae checkout with every platform built is
45-52 GB, the build volume shares its APFS pool with the main disk, and with six
bae and three coven checkouts building free space fell to 13-29 GB. So
`builds setup` also keeps a free-space floor of 30 GiB — bae's iOS build, the
largest single platform build (target-ios 12.2 GB + bae-ios/.build 4.9 GB
fresh), plus a macOS build in another checkout at the same time (7.4 + 5.2 GB).
Under the floor it first finishes removing the trash, then deletes the build
roots of checkouts that are gone, then whole folders across every checkout,
least recently written first, each one only if nothing wrote to it in the last
30 minutes (a build between two of its steps holds no lock) and it passes the
same in-use check as a cap eviction, waiting for each deletion so the next
reading counts it. If the floor is still out of reach it fails, naming the free
space and the biggest folders, so the build stops before it starts instead of
running out of space halfway. `BUILDS_FREE_FLOOR_GIB` overrides the floor.

Don't trim target dirs with `cargo sweep --time` or `--maxsize`: APFS refreshes
a file's access time only when it is not newer than its modification time (8 of
1,325 fingerprint dirs refreshed after a build), so the sweep deletes the build
needed next and leaves 14–17 GB anyway. `cargo sweep --installed` is fine after
a toolchain upgrade.

## One sccache server

- One server on sccache's default TCP port (4226), cache capped at 40 GiB in
  `~/Library/Application Support/Mozilla.sccache/config` (the working set of the
  checkouts is about 30 GiB; each checkout state writes about 3.3 GiB).
- Started by launchd (`local.sccache-server`) with the system `TMPDIR` and no
  idle timeout. A server inherits the `TMPDIR` of whichever client starts it;
  once that directory is deleted every compile fails. Don't use the Unix-socket
  mode (`SCCACHE_SERVER_UDS`): concurrent client starts raced (3 of 20 failed)
  and left orphaned servers.
- Fresh target dirs are fine: sccache does not need an existing `deps/`, and
  concurrent worktree creation does not race it. Failures blamed on that were
  one of the two causes above.
- Sharing across checkouts is partial and no setting fixes it: registry crates
  about 72% hits, git dependencies 0%, workspace crates about 29%.

Keep `split-debuginfo` unpacked (the macOS default): packed was 33% slower and
53% bigger, and off drops file:line from the top crate's frames.

## SPM / Sparkle cache recovery (Swift/Xcode + SwiftPM)

If xcodebuild fails with "Couldn't check out revision" or "file not found" on a
SwiftPM dependency (Sparkle is a frequent offender), its checkout cache is
corrupt. Clear it and rebuild:

```sh
rm -rf <app>/.build/derivedData
rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/<Dep>*
cd <app> && xcodebuild -project <proj>.xcodeproj -scheme <scheme> \
  -derivedDataPath .build/derivedData build
```

VPN can cause incomplete git fetches that corrupt the SwiftPM cache; disconnect
and retry. A prime and the scripts that build after it must use the same
xcodebuild flags (notably `-derivedDataPath`), or the primed packages land where
the later build can't find them.

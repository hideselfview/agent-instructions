# Build output per checkout, and the `builds` tool

How build output is laid out on this machine for repos with several checkouts
(the main checkout plus side worktrees `side-1`, `side-2`, …): Rust, Xcode,
Gradle, dotnet and Lean output alike. Pairs with the worktree rules in
`instructions-agent.md`; this file is the build-infra layer those rules gesture
at with "whatever per-worktree setup the project runs."

## The layout

The `builds` tool owns everything on the build volume:

```
/Volumes/Builds/<repo>/<checkout>/   one build root per checkout: main | side-N
/Volumes/Builds/<repo>/app-checkout  the name of the repo's app checkout
/Volumes/Builds/<repo>/app.lock      the lock agents hold to build there
/Volumes/Builds/DerivedData/         Xcode's default DerivedData location
/Volumes/Builds/caches/<name>/       download caches every checkout shares
/Volumes/Builds/.trash/              folders being deleted in the background
```

`builds status` lists each of these and flags anything else on the volume as
UNKNOWN (it leaves out macOS's own `.Spotlight-V100`, `.fseventsd` and the
like). Something UNKNOWN is either a new kind of build output the tool should
own or a leftover to delete; find out which.

Every checkout owns one build root at a fixed path on the build volume.

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
  can't find its packages), deletes that checkout's unused build copies, applies
  the caps to it, to DerivedData and to the shared caches, and keeps the
  volume's free-space floor (below). The repo's `scripts/build-layout.sh` calls
  it, and the hooks and every build script call that first. Without the tool
  (CI) the script does nothing and the tools write into real directories.
- `builds new [<repo>] [-b <branch>]` adds the next free `side-N` from
  `origin/main`, links it, and runs its own `post-checkout`, which sets the
  checkout up and builds nothing.
- `builds retire <checkout>` removes a clean side worktree and its build root;
  it refuses the app checkout.
- `builds app-checkout <checkout>` records a side worktree as its repo's app
  checkout (below), in `/Volumes/Builds/<repo>/app-checkout`.
- `builds status` lists every entry on the volume: each root's folders, each
  DerivedData entry and each shared cache, with size against cap, last write,
  and whether the links are in place; anything else as UNKNOWN.
- `builds prune [--dry-run] [--idle <minutes> [--include-warm]]` deletes every
  checkout's unused build copies, applies every cap, deletes roots whose
  checkout is gone and DerivedData entries nothing wrote to for a day; `--idle`
  also deletes every folder and DerivedData entry nothing wrote to for that long
  (the reclaim-disk skill uses it), except main's and the app checkout's
  folders, which `--include-warm` adds. `--dry-run` deletes nothing and prints
  what it would.
- `builds cargo <subcommand> [<args>...]` runs cargo and records the build
  copies it used (below); the repo's `scripts/cargo.sh` calls it, and the hooks
  and build scripts run cargo through that. `builds record-use` records the
  copies a cargo report on stdin names, for a script that keeps cargo's JSON for
  itself.
- `builds install` puts the tool on PATH, creates the shared caches and the
  links to them, writes the sccache config, runs the sccache server under
  launchd (restarting it only when its settings changed, since a restart fails
  every compile in flight), and points Xcode's default DerivedData
  (`IDECustomDerivedDataLocation`) at `/Volumes/Builds/DerivedData`. Run it
  again any time: it changes only what differs.

## DerivedData and the shared caches

**DerivedData.** bae's build scripts pass `-derivedDataPath` into a `.build`
folder in the checkout, so no checkout's build lives in Xcode's default
location; it holds what one-off `xcodebuild` runs (`-showBuildSettings`, package
resolves) and the Xcode app write: one `<project>-<hash>` entry per project
path, mostly SwiftPM checkouts of about 3 GB. Nothing deleted them, and nine had
piled up to 27 GB, four from worktrees already removed. It is one disposable
cache with a 15 GB cap, about five entries: `setup` and `prune` delete entries,
least recently written first, while it is over the cap; `prune` also deletes
entries nothing wrote to for a day; the free-space floor deletes its idle
entries before any checkout's folders. Deletions for the cap and the floor skip
an entry written in the last 30 minutes (a one-off run between two steps holds
no file open), and every deletion skips one a process holds a file in. A one-off
run that needs its packages again resolves them again.

**Shared caches.** Gradle, npm, NuGet, SwiftPM and sccache keep their downloads
and compiler outputs in `/Volumes/Builds/caches/<name>`, linked from where each
tool looks: `~/.gradle`, `~/.npm`, `~/.nuget`,
`~/Library/Caches/org.swift.swiftpm`, `~/Library/Caches/Mozilla.sccache`.
`builds install` creates them; a real directory already at one of those paths is
moved onto the volume whole, since it may hold the tool's settings. Each cache
has a cap, about twice its size when the caps were set (gradle 8G, npm 1G, nuget
6G, swiftpm 3G). Only the cap deletes one: `setup` or `prune` deletes a cache
over its cap, whole, once nothing has written to it for 30 minutes and no
process holds a file in it, and puts back the settings files it holds
(`gradle.properties`, Gradle init scripts, `NuGet/NuGet.Config`). The floor and
the reclaim-disk skill never touch them: deleting one makes every checkout, warm
ones included, download it all again, and a SwiftPM fetch cut off halfway
corrupts its cache. sccache keeps its own cache under the 30 GiB cap `install`
writes into its config; `builds` only shows it.

**Cargo's git checkouts.** Cargo checks out each git dependency commit it builds
into `~/.cargo/git/checkouts/<repo>-<hash>/<commit>` and never deletes one;
every coven pin bump in bae adds one, and 64 had piled up, two of them pinned.
`setup` and `prune` delete each checkout no `Cargo.lock` of a live checkout the
tool knows pins, while holding cargo's package cache locks (a cargo checking a
commit out holds them). The git databases in `~/.cargo/git/db` stay: cargo
checks an older commit out of them again in seconds when a branch needs it.

## App builds live in main and one app checkout

By default a side worktree builds host output only: for Rust, `target`, which
tests, clippy and the pre-commit hook use. App builds (an Xcode app, a Gradle
app, a bridge cross-compiled for a platform) are 10–20 GB per platform per
checkout; with every side worktree holding them, the free-space floor kept
deleting them and no app build stayed warm. A repo with app builds keeps them
warm in two checkouts:

- **main**, for the person's installs and releases.
- **The app checkout**, one long-lived side worktree, named in the project's
  instructions file (bae: `side-1`) and recorded for the tool with
  `builds app-checkout` (the name goes in `/Volumes/Builds/<repo>/app-checkout`,
  beside its lock). Agents take turns in it, one command at a time, under its
  lock.

The free-space floor (below) never deletes main's or the app checkout's folders:
deleting them is what kept every app build cold. Their caps still apply.

Another checkout may hold a warm app build while a wave of work needs it — the
person asks for an app build in a particular worktree, or parallel agents each
need one. Same judgment as extra worktrees: keep it while that work lasts, and
clear it once the work has clearly moved on (retire the worktree, or delete the
folders behind its app links while nothing builds there; the next `builds setup`
recreates them empty). The caps bound whatever a checkout builds, and the floor
bounds every checkout but main and the app checkout; they are the safety net,
not the rule.

The app checkout's working tree belongs to whoever holds
`/Volumes/Builds/<repo>/app.lock`, and only for the command they run holding it:
nobody edits there or leaves changes behind. Commit your work on your branch in
your own side worktree, then run one command that holds the lock, moves the app
checkout to your commit and builds. In bae:

```sh
lockf -k /Volumes/Builds/bae/app.lock sh -c '
  cd ~/dev/bae-worktrees/side-1 &&
  { test -z "$(git status --porcelain)" || { git status --short; exit 1; }; } &&
  git checkout -q --detach <branch> &&
  ./bae-macos/run.sh --no-open'
```

- `lockf` waits until the lock is free (`-t <seconds>` gives up after that
  long), so run it as a background task. It releases the lock when the command
  exits, crashed or not, so a lock is never left stale. `-k` keeps the lock
  file, which keeps waiters in order.
- `--detach` checks out your branch's commit: the branch itself stays checked
  out in your worktree, and git checks out a branch in one worktree only.
- To change something, edit and commit (or amend) in your worktree, then run the
  command again. Put everything that needs the build — the build, its tests,
  launching the app — in the same locked command; the next holder moves the
  checkout.
- A dirty app checkout fails the command instead of carrying someone's changes
  into your build. Find out whose they are before deleting them.

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

## Disk stays bounded by deleting unused build copies, then caps

**Unused build copies.** Cargo builds each unit (one crate target, for one
profile, target and feature set) into files named after a hash of everything
that build depends on, dependency versions included. Every coven pin bump in
bae, and every dependency bump in coven, builds a new copy of each unit above
it, and cargo never deletes the old one: bae main's `target` held 93 copies of
bae-core, 13 GB, going back a week, each a ~450 MB rlib plus ~1.4 GB of test
binaries, and the first dry run there found 17.8 of its 33 GB unused for three
days. So `builds setup` and `builds prune` first delete, in each cargo folder,
every build copy no build used for three days (`BUILDS_UNUSED_DAYS` overrides
it), and every copy a newer one replaced (below): its
`.fingerprint/<package>-<hash>`, its `deps/` and `examples/` files, its
`build/<package>-<hash>`, and the `incremental/` folders only it used, in every
profile folder cargo makes (`debug`, `release`, `<triple>/debug`, the `release`
of a `cargo install --target-dir`).

Use is recorded from cargo's own report, since a build that finds a unit up to
date writes nothing in `deps/` or `.fingerprint/` and access times can't tell
(below). The repos run cargo in their hooks and build scripts through
`scripts/cargo.sh`, which runs `builds cargo`: cargo with
`--message-format=json-render-diagnostics` (`--cargo-message-format` for
nextest), its stdout through `cargo_units.py record`; without the tool (CI), or
with a `builds` that has no `cargo` command, `scripts/cargo.sh` is cargo itself.
Each `compiler-artifact` message, for an up-to-date unit too, and each
`build-script-executed` names its unit's files; the recorder sets that unit's
fingerprint folder's modification time to now and passes every other line
through, so a `cargo run` program's output and test output still reach the
caller. Doctests have no `--no-run`, and a `--profile test` build of the library
makes a different unit than the doctests use, so reading cargo's stdout in place
is what records a doctest run. A unit's last use is the newest modification time
of its fingerprint folder and the files in it: a unit no recorded build used
counts as used when cargo last built it.

- A file cargo copies beside `deps/` (`target/debug/loc-gen`) keeps its
  original's size and modification time, which name the unit.
- Outputs whose names carry no hash, documentation and a cdylib or staticlib
  (`libbae_bridge.a`), name the units with that target, mode and features whose
  dependencies' fingerprints are among the ones the same build reported.
- An `incremental/` folder is named by rustc's crate id, not cargo's hash. It
  belongs to the unit whose `deps/` files are hard links to its files; one with
  no such link (a check of a test or a binary) goes once nothing wrote to it for
  the same days, which costs that crate a non-incremental compile.
- A build that doesn't go through `builds cargo` (cargo typed by hand,
  rust-analyzer's checks) records nothing: what only it uses goes three days
  after cargo last rebuilt it, and its next build rebuilds that, mostly from
  sccache.

**Replaced build copies.** Three days is what keeps the copies a branch from the
last few days builds against, and lowering it would delete what builds use
cargo's report doesn't record (rust-analyzer, cargo typed by hand): at one day
it would have deleted 7.6 GB of main's `target`. A copy a newer copy of the same
thing replaced goes sooner: once nothing used it for 12 hours
(`BUILDS_SUPERSEDED_HOURS`) and a copy with the same identity was used after it.
The identity is the package and target, mode, features, profile settings,
rustflags and target triple, in the same profile folder, over dependencies that
are themselves the same thing, read from each unit's fingerprint JSON and its
dependencies' fingerprints; copies with one identity differ only in their
sources (a dependency's version or git revision, the compiler). Feature variants
of a crate, which different package selections build and use side by side, have
different identities and stay. A unit a remaining unit was built against stays
too, so two versions of one crate in a single build (thiserror 1 and 2) both
stay. A workspace crate is rebuilt in place, so a dependent built before the
last rebuild names a fingerprint no unit has; such a dependency counts by its
package and crate name in every dependent. A dry run in bae main, where the
coven pin moves several times a day, found 2.7 GB of replaced copies in `target`
and 1.2 GB in `target-macos`.

**Dead object files.** With `split-debuginfo` unpacked (macOS's default), rustc
leaves each unit's object files in `deps/`, and a linked binary's debug map
names the ones it was linked from: a test's or binary's own, and the members of
each rlib it links (`libtokio-<hash>.rlib(...)`), never the rlib's loose
objects. Each build of an incremental unit, and each build of a library whose
files carry no hash (`bae_bridge.bae_bridge.<id>-cgu.NN.rcgu.o`), writes a new
set under new names and leaves the old ones; nothing deletes them while the unit
stays in use, and coven's `coven_database` lib test, rebuilt 91 times in three
hours, piled up 22 GB of them until its folder's cap deleted the whole folder.
So `setup` and `prune` also delete, in every checkout including main and the app
checkout, an rlib unit's loose objects, and any other unit's objects that its
binary or dylib doesn't name (read from the Mach-O's `N_OSO` entries when more
than one set is left). rustc reuses incremental objects from `incremental/`,
never from `deps/`, and cargo's freshness never looks at them: after deleting
them every unit stays fresh and a panic's backtrace keeps file and line, in the
test and in an rlib it calls (`test-cargo-units.sh` checks both).

Deleting holds every `.cargo-lock` of the folder's profile folders, taken
without waiting (a profile folder whose lock a build holds waits for the next
run), then lists open files once and keeps every unit a process holds a file of;
rust-analyzer's proc-macro server holds its macro dylibs. Open files are matched
by inode: lsof names a hard-linked file by whichever of its names it finds, and
rustc hard-links `deps/` files into `incremental/`. A unit's fingerprint goes
first: without it cargo rebuilds the unit before reading its outputs, so a
deletion cut off halfway leaves only files the unit's next build overwrites.

**Caps.** A folder still over its cap is deleted whole (renamed into
`/Volumes/Builds/.trash`, removed in the background, recreated empty), unless a
build is using it. Hooks and build scripts in one checkout overlap, so "in use"
is decided while holding what a build holds: every `.cargo-lock` flock in the
folder, taken without waiting (cargo holds one for a whole build), then a fresh
check that no process has a file or working directory inside it, then the
rename, then the locks are released. A cargo build that starts meanwhile queues
on the lock and writes into the new folder. Caps are about twice a fresh build
of the folder; the next build refills it, mostly from sccache (a wipe and
rebuild took 137 s and 8.9 GB). Sizes, for the caps and `builds status`, count
each block once (`disk_usage.py`): cargo uplifts a library out of `deps/` as an
APFS clone, which shares its blocks, and `du` counts those blocks once per file,
so it read bae's `target-macos` as 9.3 GB where it took 7.3 GB.

Caps bound folders, not the volume: a bae checkout with every platform built is
45-52 GB, the build volume shares its APFS pool with the main disk, and with six
bae and three coven checkouts building free space fell to 13-29 GB. So
`builds setup` also keeps a free-space floor of 30 GiB — bae's iOS build, the
largest single platform build (target-ios 12.2 GB + bae-ios/.build 4.9 GB
fresh), plus a macOS build in another checkout at the same time (7.4 + 5.2 GB).
Under the floor it first finishes removing the trash, then deletes the build
roots of checkouts that are gone, then DerivedData's entries, then whole folders
across every checkout but main and the app checkout, least recently written
first, each one only if nothing wrote to it in the last 30 minutes (a build
between two of its steps holds no lock) and it passes the same in-use check as a
cap eviction, waiting for each deletion so the next reading counts it. It never
deletes a shared cache. If the floor is still out of reach it fails, naming the
free space, the biggest folders it may delete and the size of main's and the app
checkout's, so the build stops before it starts instead of running out of space
halfway. It never falls back to main's or the app checkout's folders: the person
decides what to free there. `BUILDS_FREE_FLOOR_GIB` overrides the floor.

Access times can't say what a build used: APFS refreshes a file's access time
only when it is not newer than its modification time (8 of 1,325 fingerprint
dirs refreshed after a build), so `cargo sweep --time` or `--maxsize` deletes
the build needed next and leaves 14–17 GB anyway. `cargo sweep --installed` is
fine after a toolchain upgrade.

## One sccache server

- One server on sccache's default TCP port (4226), cache in
  `/Volumes/Builds/caches/sccache` and capped at 30 GiB in
  `~/Library/Application Support/Mozilla.sccache/config`, both written by
  `builds install`. Each checkout state writes about 3.3 GiB; a day of agents
  building in seven bae and five coven checkouts wrote 12.6 GiB, so 30 GiB keeps
  about two days of branch states.
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

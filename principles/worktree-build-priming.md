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

The tool is `builds`, a Rust program in
`~/dev/agent-instructions/scripts/builds`. `install.sh` builds it and
`builds install` copies it onto PATH as `~/.local/bin/builds` in one rename, so
a failed build leaves the installed one as it was; a commit on main that touches
`scripts/builds/` runs `install.sh` again:

- `builds setup [<checkout>]` creates the links, recreates any folder behind a
  link that was deleted, deletes a folder an older layout left behind (never
  moves it: Xcode's DerivedData records its own absolute path, so a moved one
  can't find its packages), deletes that checkout's unused build copies, applies
  the caps to it, to DerivedData and to the shared caches, deletes idle build
  output in the temporary folders, and keeps the volume's free-space floor
  (below). The repo's `scripts/build-layout.sh` calls it, and the hooks and
  every build script call that first. Without the tool (CI) the script does
  nothing and the tools write into real directories.
- `builds new [<repo>] [-b <branch>] [--for <who>]` adds the next free `side-N`
  from `origin/main`, links it, and runs its own `post-checkout`, which sets the
  checkout up and builds nothing; `--for` claims it.
- `builds claim <checkout> <who>` and `builds release <checkout>` record and
  remove who works in a side worktree. Whoever hands a worktree out (a
  coordinator dispatching an agent, a person) claims it, and releases it when
  the work lands or moves on; an agent works in the worktree its brief names, or
  one it gets from `builds new --for`, never a side-N picked by hand. The claim
  is a file in the worktree's git dir, shown by `status` and refused by
  `retire`. It isn't a lock: an agent is a series of separate commands, so
  nothing could hold one, and a claim left by a session that ended shows its age
  in `status`. One build at a time stays a real lock (`app.lock`).
- `builds note <checkout> <text>` sets a one-line note on the side worktree's
  claim, where its work stands ("waiting on Linux CI"); `builds note <checkout>`
  clears it. The note is a third tab-separated field on the claim's line, so it
  goes with the claim; claiming the worktree again for the same who renews the
  claim's date and keeps the note. `status` prints it under the checkout. A
  claim whose who ends in `(landing)` (`coordinator (landing)`) marks the
  worktree its coordinator lands commits from.
- `builds note <checkout> --state waiting-on-you|winding-down|held <text>` sets
  the note with a state the dashboard can't read from git: `waiting-on-you` (a
  question only the person can answer, or a step only they can take),
  `winding-down` (the agent's last item, then its handoff) or `held` (the claim
  is kept on purpose with no work running, such as uncommitted work waiting for
  another change to land; the note says why and until when). `health` doesn't
  flag a held claim as idle, and the dashboard shows it as *Held* with its note.
  The state is a fourth field on the claim's line and needs the note's text; a
  plain `note` sets the note without one, so a coordinator clears the state by
  writing the next note. `status` prints it as `note (waiting on you): <text>`,
  and `status --json` as the claim's `note_state` (null without one).
- `builds retire <checkout>` removes a clean, unclaimed side worktree and its
  build root; it refuses the app checkout.
- `builds app-checkout <checkout>` records a side worktree as its repo's app
  checkout (below), in `/Volumes/Builds/<repo>/app-checkout`.
- `builds status` lists every entry on the volume: each root's folders, each
  DerivedData entry and each shared cache, with size against cap, last write,
  and whether the links are in place; anything else as UNKNOWN. It also lists
  build output in the temporary folders. `builds status --json` prints the
  repos' checkouts (path, branch, app checkout, claim and note, each build
  folder's size against cap, last write and link) and the volume's free space
  against both floors, as JSON; `--no-folders` leaves out the build folders. A
  folder's last write is the newest write anywhere inside it. While the
  dashboard's daemon runs, `status` takes every size and last write from it
  (`status --json` in 0.2 s, where walking the volume took 19 s at load 40);
  without it, `status` walks the folders itself, eight threads at a time: the
  walk is system calls on many small files, which eight threads finish several
  times faster than one. Commands that delete (`setup`, `prune`, the floor) ask
  the daemon too, but first wait for it to have seen every write made before the
  question, so a size measured after a deletion counts it.
- `builds health [--json]` says what is stuck or needs acting on: load, free
  space against both floors, whether sccache finished compiles since the last
  readings while clients wait 10+ minutes, Claude shells older than 2 hours,
  each claimed worktree (claim, branch, uncommitted files, commits waiting to
  land, minutes since a change, build processes there) and each app lock's
  holder; it exits 1 when something needs acting on. It asks the daemon when one
  runs and reads the machine itself otherwise; it never changes anything.
- `builds dashboard [--port <port>]` runs the daemon until it is killed. It
  follows the volume through FSEvents, walking it whole only at its start or
  when FSEvents says it dropped events (476k entries in about 4 s, about 100 MB
  resident) and a folder again when FSEvents asks for that, and keeps every
  folder's size and last write current, each block counted once and each
  hard-linked file once. It follows every checkout's folder and git dir the same
  way and reads a checkout's git state, claim and manifest again only when
  something there changed. It answers the commands' questions on
  `/Volumes/Builds/.builds-daemon/socket` and serves a page called Builds at
  `http://127.0.0.1:7787/` (only on 127.0.0.1, only to a Host of 127.0.0.1 or
  localhost) showing every checkout `status` lists. Its header shows the load
  with a line of the last ten minutes against the core count, the volume's free
  space as a bar against the 30 GiB warning floor and the 10 GiB hard floor, and
  whether the page is live; under it, what `health` says needs acting on. Each
  repo pins its main checkout and its app checkout ("App build") first, each
  with its total build output and its 3 largest build folders (short names such
  as "Xcode build" and "Android app", the path in the chip's tooltip; the rest
  behind a "+N more folders" disclosure), and for the App build whether its
  `app.lock` is free or held, the command holding it and how many wait, read
  without taking the lock (the process with the file open holds it, since
  `lockf` waits inside `open`). Every other side worktree is a card, as tall as
  its content: name, branch, state, the claim's who and note, the commits
  waiting to land, its last commit's age, the count of uncommitted files, its
  build size (flagged over a cap) and the claim's date. A card where builds run
  says which (`cargo ×3, rustc`), with their CPU and memory.
  - **States.** Each side worktree gets one, the first that matches: *Waiting on
    you* (the note's state says so), *Landing* (the claim's who ends in
    `(landing)`), *Held* (the note's state says so), *Ready to land* (commits
    waiting to land, claimed or not: unclaimed ones are work an agent left
    behind), *Winding down* (the note's state says so), *Working* (claimed),
    *Free* (unclaimed). A commit waits to land when `origin/main` has no commit
    with the same change
    (`git log --cherry-pick --right-only origin/main...HEAD`): a coordinator
    lands by rebasing or cherry-picking, so a branch's own commits stay off
    `origin/main` by hash after they land.
  - **Two views,** switched by the two buttons in the header, the choice kept in
    the browser: *By repo* (the default), a section per repo with its pinned
    cards and then its worktrees, sorted by state; and *Lanes*, the pinned cards
    in one row and then every worktree in a lane: Waiting on you, Ready to land,
    Working (with Winding down, Held and Landing tagged), Free.
  - **Order and quiet.** Within each state (and each lane) the worktrees sort by
    last activity, newest first: the newer of HEAD's commit time and the newest
    modification time among the uncommitted files (`status --porcelain -z`, each
    listed path `lstat`ed; a directory listed as untracked counts by its own
    time). A card shows "last write Nm ago" beside "last commit", and a worktree
    with uncommitted files and no write in 30 minutes gets a muted "quiet" mark,
    so a stalled agent stands out. The stats add about 2 ms to side-5's 177
    changed files, whose `git status` takes 30-60 ms. When the app lock is held,
    the page shows the holder's command as its script's file name, the full
    command in the tooltip. The pinned row's two cards share one height.
  - **Hide free worktrees.** A checkbox in the header, kept in the browser like
    the view, leaves out the Free worktrees (unclaimed, nothing waiting to land)
    in both views: their cards in *By repo* and the Free lane in *Lanes*. The
    header then says how many are hidden ("3 free hidden"). Main and the App
    build stay shown, and an unclaimed worktree with commits waiting to land is
    *Ready to land*, so it stays shown too.
  - **Live.** The page asks for nothing: it holds one `/events` stream
    (server-sent events), and the daemon sends the whole state on it, at most
    four times a second, whenever anything in it changed, waiting 300 ms after a
    change for the rest of a burst of writes. Cards are replaced only when what
    they show changed, so an open disclosure stays open. While a page watches,
    the daemon reads the load and the processes every 2 seconds (with no page
    open it reads none) and asks sccache for its count every 30 seconds.
    `/api/state` answers the same state once. The daemon runs git only in side
    worktrees, read-only with `--no-optional-locks`, and fetches nothing: for
    the person's main checkouts it shows what `status` shows. Coordinators keep
    it current through the claims and notes they already make, setting a note's
    state when the work waits on the person or is winding down. `test-claims.sh`
    tests claims, notes and their states, `status --json`, the daemon's answers,
    its events and the states it derives on a scratch repo and build volume.
- `builds prune [--dry-run] [--idle <minutes> [--include-warm]]` deletes every
  checkout's unused build copies, applies every cap, deletes roots whose
  checkout is gone, DerivedData entries nothing wrote to for a day and build
  output in the temporary folders nothing wrote to for 30 minutes; `--idle` also
  deletes every folder and DerivedData entry nothing wrote to for that long,
  except main's and the app checkout's folders, which `--include-warm` adds.
  `--dry-run` deletes nothing and prints what it would.
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
(`gradle.properties`, Gradle init scripts, `NuGet/NuGet.Config`). The floor
never touches them: deleting one makes every checkout, warm ones included,
download it all again, and a SwiftPM fetch cut off halfway corrupts its cache.
sccache keeps its own cache under the 30 GiB cap `install` writes into its
config; `builds` only shows it.

**Cargo's git checkouts.** Cargo checks out each git dependency commit it builds
into `~/.cargo/git/checkouts/<repo>-<hash>/<commit>` and never deletes one;
every coven pin bump in bae adds one, and 64 had piled up, two of them pinned.
`setup` and `prune` delete each checkout no `Cargo.lock` of a live checkout the
tool knows pins, while holding cargo's package cache locks (a cargo checking a
commit out holds them). The git databases in `~/.cargo/git/db` stay: cargo
checks an older commit out of them again in seconds when a branch needs it.

**Build output in the temporary folders.** A build pointed at `/private/tmp` or
the user's temporary folder (`getconf DARWIN_USER_TEMP_DIR`, where `mktemp` puts
things) — `CARGO_TARGET_DIR=/tmp/...`, `-derivedDataPath /tmp/...`, a probe
crate or package built where it was made — writes 5–14 GB there that nothing
else deletes; before `builds` covered them such folders piled up past 100 GB.
Nothing on this machine points a build there any more (the shell profile,
agent-instructions, the repos' scripts and the Claude Code and Codex settings
were checked on 2026-11-04): build in the checkout, whose paths lead to the
volume. `setup` and `prune` delete each cargo target folder (`.rustc_info.json`
at its top), Xcode DerivedData folder (`info.plist` beside `Logs` and `Build` or
`ModuleCache.noindex`) and SwiftPM folder (`workspace-state.json` beside
`checkouts`, `repositories` or `artifacts`) found there, four levels deep at
most, that nothing wrote to for 30 minutes and no process holds a file in
(`status` shows them). They skip `/private/tmp`'s `claude-*` (Claude Code
sessions' scratch, whose entries go after three days), `com.apple.*` (launchd's
per-session socket folders) and `tmux-*` (the tmux server's socket: deleting it
leaves the running server unreachable), and anything the user doesn't own.

## rust-analyzer builds into a folder of its own

The Claude Code session's rust-analyzer comes from agent-instructions' own
plugin, `claude/plugins/rust-analyzer` (a folder marketplace, in place of
`rust-analyzer-lsp@claude-plugins-official`), which starts it with
`initializationOptions` `cargo.targetDir = "target-ra"` and
`checkOnSave = false`:

- **Its own folder.** rust-analyzer's build-script and proc-macro step is a
  `cargo check --workspace` of everything, and with check-on-save it checked the
  workspace again on each change of the files it watches: in bae main, every
  landing, which held main's `target` over its cap (39G of 30G) and kept the
  volume under the free-space floor. `target-ra` is a `.build-dirs` entry with
  its own cap in bae and coven; a Rust repo without that entry gets a real
  `target-ra` folder in its checkout.
- **No check-on-save.** The session uses rust-analyzer to navigate; the
  pre-commit hook and clippy catch compile errors. Build scripts and proc macros
  still build, since navigation needs them.
- **Options at initialization, not in a `rust-analyzer.toml`.** In rust-analyzer
  1.95.0 the build-script step, flycheck and the first check read their settings
  with no source root (`cargo(None)`, `flycheck(None)`), which skips a repo's
  `rust-analyzer.toml`, and the user's own `rust-analyzer.toml` is read only
  after the first build-script step has started into `target`. Options passed at
  initialization apply before anything builds.

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
nextest), its stdout read by the tool as cargo writes it; without the tool (CI),
or with a `builds` that has no `cargo` command, `scripts/cargo.sh` is cargo
itself. Each `compiler-artifact` message, for an up-to-date unit too, and each
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

**Idle incremental folders.** Cargo never deletes an
`incremental/<crate>-<hash>` folder of an old hash (another feature set, older
sources, the dev profile clippy, check and the hook use), so they fill a folder
up to its cap: coven side-3's `target` stood at 44.9 GB under a 45 GB cap, 28 GB
of it `debug/incremental`, and 112 of its 162 folders had not been written in
two hours. So `setup` and `prune` also delete, in every profile folder of every
checkout (main's and the app checkout's too), each incremental folder whose
newest write anywhere inside it is more than six hours old
(`BUILDS_INCREMENTAL_HOURS` overrides it), whether or not its unit is still in
use. Losing one costs an incremental rebuild of its crate, mostly from sccache.
It runs under the same guard as the rest of the prune: a profile whose cargo
lock a build holds is skipped, and so is a folder a process holds a file in. The
line it prints gives the count, the size freed (not counting `deps/` files the
folder hard-links, which stay) and the three largest; a dry run
(`builds prune --dry-run`) prints what it would delete.

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
build is using it or its worktree is claimed: a claimed worktree's agent is
mid-task, so the build reports the folder over its cap and keeps it. Hooks and
build scripts in one checkout overlap, so "in use" is decided while holding what
a build holds: every `.cargo-lock` flock in the folder, taken without waiting
(cargo holds one for a whole build), then a fresh check that no process has a
file or working directory inside it, then the rename, then the locks are
released. A cargo build that starts meanwhile queues on the lock and writes into
the new folder. Caps are about twice a fresh build of the folder; the next build
refills it, mostly from sccache (a wipe and rebuild took 137 s and 8.9 GB).
Sizes, for the caps and `builds status`, count each block once
(`builds du <path>...` prints them): cargo uplifts a library out of `deps/` as
an APFS clone, which shares its blocks, and `du` counts those blocks once per
file, so it read bae's `target-macos` as 9.3 GB where it took 7.3 GB. A file
with several hard links (rustc links `deps/` files into `incremental/`) counts
once.

Caps bound folders, not the volume: a bae checkout with every platform built is
45-52 GB, the build volume shares its APFS pool with the main disk, and with six
bae and three coven checkouts building free space fell to 13-29 GB. So
`builds setup` also keeps a free-space floor of 30 GiB — bae's iOS build, the
largest single platform build (target-ios 12.2 GB + bae-ios/.build 4.9 GB
fresh), plus a macOS build in another checkout at the same time (7.4 + 5.2 GB).
Under the floor it first finishes removing the trash, then deletes the build
roots of checkouts that are gone, then DerivedData's entries, then whole folders
of unclaimed side worktrees, least recently written first, each one only if
nothing wrote to it in the last 30 minutes (a build between two of its steps
holds no lock) and it passes the same in-use check as a cap eviction, waiting
for each deletion so the next reading counts it. A claimed worktree's folders
are never deleted here: the agent working in it can go an hour between builds
while it reads and edits, and losing its folder costs it a cold rebuild of every
dependency in the middle of its task (bae side-5's 14.9 GB target was deleted
that way on 2026-11-04, and its next checks took over two hours under load). It
never deletes a shared cache. If the floor is still out of reach it warns,
naming the free space, the biggest folders it may delete and the size of main's
and the app checkout's, and the build goes on: a commit's hook stalling an agent
mid-work is the wrong place to manage disk; whoever runs the agents sees the
warning in their reports, stops starting more and retires one at a clean point.
Only under a hard floor of 10 GiB, about one platform build, does it fail, so
the build stops before it starts instead of running out of space halfway, which
would also break the person's apps on the shared pool. It never falls back to
main's or the app checkout's folders: the person decides what to free there.
`BUILDS_FREE_FLOOR_GIB` and `BUILDS_HARD_FLOOR_GIB` override the two floors.

Access times can't say what a build used: APFS refreshes a file's access time
only when it is not newer than its modification time (8 of 1,325 fingerprint
dirs refreshed after a build), so `cargo sweep --time` or `--maxsize` deletes
the build needed next and leaves 14–17 GB anyway. `cargo sweep --installed` is
fine after a toolchain upgrade.

## One sccache server

- One server on sccache's default TCP port (4226), cache in
  `/Volumes/Builds/caches/sccache` and capped at 30 GiB in
  `~/Library/Application Support/Mozilla.sccache/config`, both written by
  `builds install`. A tool change to the cap reaches the config only when
  `install` runs again, which restarts the server; `builds status` flags a
  config that no longer holds the tool's cap. Each checkout state writes about
  3.3 GiB; a day of agents building in seven bae and five coven checkouts wrote
  12.6 GiB, so 30 GiB keeps about two days of branch states.
- Started by launchd (`local.sccache-server`) with the system `TMPDIR` and no
  idle timeout. A server inherits the `TMPDIR` of whichever client starts it;
  once that directory is deleted every compile fails. Don't use the Unix-socket
  mode (`SCCACHE_SERVER_UDS`): concurrent client starts raced (3 of 20 failed)
  and left orphaned servers.
- Fresh target dirs are fine: sccache does not need an existing `deps/`, and
  concurrent worktree creation does not race it. Failures blamed on that were
  one of the two causes above.
- Sharing across checkouts is partial and no setting of sccache 0.13 fixes it.
  Measured 2026-11-05 (coven-database built in two fresh worktrees of one
  commit, empty cache): the second worktree hit 92 of 126 cacheable compiles
  (73%). The 34 misses are crates that read `OUT_DIR` (serde_core, thiserror,
  libsqlite3-sys, ...) or depend on a proc-macro crate, whose dylib is rebuilt
  per checkout and never cached. `SCCACHE_BASEDIRS` exists since 0.14.0 but
  strips paths for C/C++ only until Rust support (PR #2918, merged to main
  2026-11-02) ships; 0.19.0, the newest release, lacks it. It also needs one
  prefix per checkout root in the server's config (a restart), so adopting it
  waits for a release that has it.
- What sccache never caches, by sccache's own reason: `crate-type` (build
  scripts and proc-macros, 213 of 800 requests in a coven check/clippy/test
  cycle), `incremental` (coven's workspace crates, whose profile has incremental
  on) and `multiple input files` (every workspace crate clippy lints:
  `sccache clippy-driver rustc ...` reads the rustc path as a second input). The
  last is correct: a lint run has to run.
- The cache held 19.5 hours of writes at 30 GiB on 2026-11-05 (18.9k entries),
  not two days; 70% of its bytes are workspace-crate states of a unit that
  already has an entry.
- Incremental on or off (edit-and-recheck loop, user+sys CPU seconds, one added
  constant; `clippy --all-targets`): coven off 84 / 28 (edit in foundation / in
  sync), on 27 / 10; bae off 20 / 10 (edit in bae-db / bae-library), on 10 / 4.
  sccache never sees clippy's workspace crates, so incremental off buys a hit
  only when a worktree returns to an earlier source state for build or test. Do
  not set `CARGO_INCREMENTAL=1` in the environment: sccache then refuses every
  compile; use `profile.dev.incremental` in Cargo.toml.
- `[profile.<p>.package."*"]` beats `[profile.<p>.build-override]`: with the
  `"*"` override at opt-level 3, build scripts, proc-macros and their host
  dependencies still compile at opt-level 3 whatever `build-override` says
  (measured: 24 build scripts and 8 proc-macros stayed at 3).

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

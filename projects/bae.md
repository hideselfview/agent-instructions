# bae

bae is a personal music library and player. A Rust core reads and writes the
library and does all the work. Native apps on each platform show the library
through a generated bridge. coven syncs the library between devices through S3,
Dropbox, OneDrive or iCloud.

## Crates and apps

bae's Rust code is a workspace of crates:

- bae-app is the top crate. It uses bae-library and one crate per domain, such
  as bae-records and bae-import.
- bae-bridge gives the apps the core through uniffi.
  - Its `Bridge*` records and enums are defined in Rust in
    `bae-bridge/src/types/`.
  - The build generates their Swift and Kotlin versions, which are gitignored.
  - To check a bridge type's fields, or whether an app type duplicates one, read
    `bae-bridge/src/types.rs` and its modules.

bae has four apps:

- bae-macos is the macOS app, with the shared Swift package BaeKit. It is in
  development.

- bae-android is the Android app. It is in development.

- bae-ios is the iOS app. It is paused until macOS and Android are done: it is a
  launch shell with no screens.

  - Don't build anything for iOS locally: not the app, the bridge, or BaeKit.
  - Don't add iOS screens or iOS versions of macOS views. Work that would touch
    iOS stops at macOS and Android.
  - CI's iOS jobs still run. When one fails, fix it then.

- bae-avalonia is the Windows and Linux app. It is paused until macOS and
  Android are done: it is a launch shell with no screens.

  - Don't add Windows or Linux screens.
  - When a bridge shape changes, bae-avalonia only has to compile against the
    regenerated bindings and pass its smoke test (`bae-avalonia/smoke-test.sh`
    on macOS).
  - CI's Windows and Linux jobs still run. When one fails, fix it then.

## Writing and reading the database

bae's library is one SQLite database on each device, which bae runs through
coven, a separate repo (`~/dev/coven`). coven syncs the database's rows between
devices through the person's cloud storage, merges writes that devices made
while offline, and gives bae live queries: SQL queries that coven reruns
whenever a write changes what they read.

### bae writes as if it were the only device

bae writes as though every device wrote to this one database, one write at a
time, in plain local SQL. Nothing in bae's schema, triggers or code is about
other devices, writes that arrive later, devices that were offline, or the order
in which remote rows apply. coven handles concurrent writes, rows whose parent
another device deleted, and conflicts. If a bae write cannot be written as
local, coven lacks something: fix coven, and never work around it in bae.

### Each domain owns its tables

Each table is in the crate of the domain that owns it: records are in
bae-records and the import's tables in bae-import, for example. bae-store has
none. Only a domain's own code writes its tables, and tests follow the same
rule. "Changing a table" below says where the SQL goes and how the rule is
checked.

### Shapes change in one commit, with no compatibility code

This holds in bae and coven, for the sync, encryption and cloud storage formats
as much as for tables and bridge types
(`rules/necessary-code/change-a-shape-in-one-commit-with-no-compatibility-code.md`).

Never mention a reset to the person, even when a change that needs one lands.

### A screen shows its query's rows

Rust turns the rows of a screen's SQL query into bridge types, and the app draws
them, with no cache, enrichment or other step between the query and the screen.
Only a value that changes many times a second travels another way, on a keyed
latest-value stream, and state that exists only on screen, such as hover or a
drag in progress, stays in the view that controls it. The full rules are
`projects/bae-rules/screen-reads/reactive-screens-read-plain-live-queries.md`
and `projects/bae-rules/ui-views/native-views-draw-fast-changing-values.md`.
Every plan, brief and review checks a change's reads and fast values against
these rules. A plan that proposes a shape they forbid is wrong: raise it instead
of building it.

## Building and testing

The `builds` tool manages checkouts, build folders and app builds
(`builds help`, `builds help workflow`).

### Checkouts

`~/dev/bae` and `~/dev/coven` are the person's main checkouts, where no agent
works (`builds help workflow WORKTREES`).

### Rust builds

Run every cargo command through `scripts/cargo.sh`, which runs `builds cargo`.
`builds help CARGO` says which subcommands builds refuses, which host runs each
command, and how to keep a test on this Mac.

### App builds

A change to Swift, Kotlin, C# or a bridge shape the apps compile against needs
an app build. Commit on your branch, then run this as a background task:

```sh
builds app-build bae <branch> [--test <Class>]... [--shots <prefix>]...
```

Name with `--shots` the screens the change touches, such as `mac.welcome.`.
`builds help APP BUILDS` says which prefixes builds refuses and how it picks
what to build and where, and `builds help workflow BUILDS` says who runs every
shot of a platform. CI takes no shots: only app builds run them.

The Mac shots take each window in an area of 1600 by 1200 points at two pixels
per point, and the renderer fails a scene whose window is larger
(`bae-macos/bae/baeShots/ShotDisplay.swift`).

The `ci-only` line of `.app-build` names the whole test suites
(`macos-tests-run`, `macos-tests-all`, `android-tests`), which only CI runs.

Notarized macOS releases come only from `main`, and `scripts/release-macos.sh`
builds them on CI.

## Making a change

### Every commit keeps the spec current

`spec/bae.md` states what bae does. The spec is a draft, and the coding rules
outrank it. Every commit checks whether it changes or adds anything the spec
states or should state (behavior, a schema or sync shape, a user-visible rule),
and if it does, edits the spec in the same commit. A commit without its spec
edit is unfinished
(`rules/commits-and-repo-contents/every-change-agrees-with-its-product-spec.md`).

### Changing a table

A domain's SQL files are in the same folder as a `.rs` file of the same name,
with their trigger tests in `<stem>_tests.rs`. The schema is one migration,
which bae-library assembles (`SCHEMA`) from each crate's parts in crate order. A
schema change edits its domain's SQL file and never adds a second migration. No
library has to advance from an older schema: `scripts/reset-bae.py` resets
development data.

The pre-commit hook checks that only a domain's own code writes its tables: it
runs coven's structural checker (`scripts/owner-construction-check.sh`), whose
`database-boundary` rule refuses any crate that writes a table
`owner-construction-check.toml` doesn't assign to it.

A test builds the library state it needs through the owning domain's own writes,
never by writing tables directly. When a test needs damaged data, such as a
value a CHECK refuses or a write that fails, a named function in the owning
domain writes it, and a test that must open a library file directly does so only
through `plant_in_library_file` in `bae_store::test_fixtures::planted_damage`.
bae-store gives no `Connection` to other crates, so a domain's tests open their
own in-memory library with `Schema::open_bare_library(&conn)`.

### Moving the coven pin

bae's `Cargo.toml` pins coven to one commit (`rev`). Move the pin as soon as
coven lands what bae needs. While coven lands a planned series of commits, move
the pin once, after the series.

To move the pin, change `rev` in `Cargo.toml` and run
`scripts/cargo.sh update -p coven`, which rewrites the `Cargo.lock` entries of
every coven crate. Commit both files.

### File size

A source file must not exceed 1,000 lines (`scripts/check-source-file-size.py`,
with its frozen per-file exceptions). A Rust file over 1,000 lines keeps its
`#[cfg(test)]` module in a sibling `_tests.rs` file. Split production code along
its domain and ownership boundaries. Don't expose an owner's state or duplicate
types to make a split compile.

### Checking a change

Check only what the change touches: the pre-commit hook and the tests of the
changed code (`builds help workflow WHAT AN IMPLEMENTING AGENT DOES`, step 5).
Test a crate with the features the hook builds it with, so the test reuses the
hook's build instead of compiling every crate again:
`scripts/cargo.sh test -p <crate> --features test-utils <filter>` for a crate
that declares `test-utils`, no features for one that doesn't, and
`--features oauth-providers,cloudkit,cast,test-keyring,dev-reset` for
bae-bridge. Use another feature set only to check that feature set.

Tests, benches and probes never write to the person's keychain: use an in-memory
keychain (`Coven::in_memory`) or a temporary one. An entry in the synced iCloud
Keychain reaches every device on the person's Apple account. Read the person's
keychain only when they ask, for that task.

## How work is tracked

The person decides what bae does. A coordinator session, its agents, a judge
session and a design session do the work (`builds help workflow`, ROLES).

### Board matching

While bae's main checkout holds `plans/board-matching.md`, the coordinator runs
that plan round by round and starts no other work unless it blocks one of the
round's landings.

### Decision stack and work queue

bae and coven share the lists of project bae in the `builds` store.
`builds help lists` says what each list holds, who writes what, and the order in
which the person answers the stack.

Work on the builds tool goes first in the queue (add it with
`--priority first`): every agent works through builds, so what it lacks slows
every agent.

### Items and briefs

An item with an implementation brief names its file, `plans/briefs/<area>.md` in
the main checkout (git ignores `plans/`).

## Logs and known build failures

- The macOS app logs to the unified log under subsystem `fm.bae.desktop`, and
  Debug builds keep every level.

- xcodebuild fails with "Couldn't check out revision" or "file not found" on
  Sparkle when the SwiftPM cache is corrupt. `builds help` (DISK) gives the
  steps that repair the cache.

- bae-audio finds FFmpeg through `FFMPEG_DIR` and its tests find FFmpeg's
  libraries through `DYLD_LIBRARY_PATH`, which `.cargo/config.toml` sets, and
  `scripts/prepare-ffmpeg.sh` says how each checkout gets its FFmpeg. Don't
  export `FFMPEG_DIR` or `DYLD_LIBRARY_PATH`: macOS drops `DYLD_*` whenever a
  shell script starts, so an export before `scripts/cargo.sh` never reaches the
  tests.

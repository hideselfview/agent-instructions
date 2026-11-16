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
There are two exceptions:

- A value that changes many times a second travels on a keyed latest-value
  stream, as a channel would carry it inside Rust. Some such values live in
  Rust's memory, such as playback position or a transfer's bytes. A Rust watcher
  in the value's domain computes others from an in-memory value and a live
  query's latest result: an upload bar is the release's stored bytes, from the
  query, plus the bytes of its files that bae has sent so far. Core never writes
  such a value to a table for a query to read, and never puts a plain query
  result on a fast stream.
- State that exists only on screen (hover, focus, the selected rows, an open
  popover or menu, a drag in progress, the text typed in an input field) stays
  in the view that controls it. A drag or click that writes stays in progress
  until its write returns, so the view draws the moved row or new value until
  then and the screen never jumps back. When a query's parameters change, such
  as a search's text, the view keeps drawing the last rows until the query
  returns its first rows for the new parameters.

The full rules are
`projects/bae-rules/screen-reads/reactive-screens-read-plain-live-queries.md`
and `projects/bae-rules/ui-views/native-views-draw-fast-changing-values.md`.
They load only when an agent edits matching files, so every plan, brief and
review checks a change's reads and fast values against them. A plan that
proposes a shape they forbid is wrong: raise it instead of building it.

## Building and testing

The `builds` tool manages checkouts, build folders and app builds
(`builds help`, `builds help workflow`).

### Checkouts

`~/dev/bae` and `~/dev/coven` are the person's checkouts. No agent edits,
builds, commits or cherry-picks there, the coordinator included. Agents work in
side worktrees, and the dashboard (http://127.0.0.1:7787/) shows who has each
one.

### Rust builds

Run every cargo command through `scripts/cargo.sh`, never bare `cargo`
(`builds help`, CARGO). `scripts/cargo.sh test`, `clippy`, `check`, `doc` and
`nextest` run on this Mac or the M1 (a second Mac that builds also uses),
whichever has more free cores. A test that needs this Mac's keychain, a local
server or a file outside the checkout fails on the M1: run it with
`BUILDS_CARGO_HOST=local`.

Don't delete build folders yourself: `builds` deletes the ones no build has used
for days and keeps the rest under their size caps.

### App builds

A change to Swift, Kotlin, C# or a bridge shape the apps compile against needs
an app build. Commit on your branch, then run this as a background task:

```sh
builds app-build bae <branch> [--test <Class>]... [--shots <prefix>]...
```

`--shots <prefix>` runs the shots (pictures of app views, each checked against
its board) whose names start with the prefix. Name the screens the change
touches, such as `mac.welcome.`: builds refuses `mac.` and `android.`, which
select every Mac shot or every Android shot, and CI runs every shot.
`builds help` (APP BUILDS) says how builds picks what to build and where.

The `ci-only` line of `.app-build` names the steps that run a whole test suite
or a fixed set of shots (`macos-tests-run`, `macos-tests-all`, `android-tests`,
`mac-shots`, `mac-window-shots` and `android-shots`). builds refuses them, and
CI runs them. To build something else in an app checkout, add a `step` line and
an `on` line to `.app-build`.

Notarized macOS releases come only from `main`, and `scripts/release-macos.sh`
builds them on CI.

### iOS simulators

Keep at most one iOS simulator runtime and one simulator device installed. When
you install a new runtime, delete the old runtime and its devices
(`xcrun simctl runtime list`, `xcrun simctl runtime delete`,
`xcrun simctl delete`). Each runtime takes 8 to 20 GB of the disk the builds
share.

## Making a change

### Every commit keeps the spec current

`spec/bae.md` states what bae does. Every commit checks whether it changes or
adds anything the spec states or should state (behavior, a schema or sync shape,
a user-visible rule), and if it does, edits the spec in the same commit. A
commit without its spec edit is unfinished.

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

### File size

A source file must not exceed 1,000 lines (`scripts/check-source-file-size.py`,
with its frozen per-file exceptions). A Rust file over 1,000 lines keeps its
`#[cfg(test)]` module in a sibling `_tests.rs` file. Split production code along
its domain and ownership boundaries. Don't expose an owner's state or duplicate
types to make a split compile.

### Checking a change

Before you commit, run the pre-commit hook (the `.checks` lines the staged files
reach: formatters and static checks only) and the tests of the components the
change affects. Then commit and push. When a CI job fails, reproduce that job
locally.

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

The person decides what bae does. A coordinator session, its agents and a design
session do the work (`builds help workflow`, ROLES).

### Decision stack and work queue

bae and coven keep two lists in the `builds` store, never in a file or a commit
(`builds help lists` has the commands):

- The decision stack holds what waits on the person: questions, work only they
  can do, and work held until they start it.
- The work queue is the task list for the coordinator and its agents: only what
  they can do.

Nothing to do is kept anywhere else (not in a brief, a handoff, a plan, the
decision log or a chat message) unless an item points at it. Commits, code and
repo files never name list ids or the lists. Neither list holds what is not
relevant now, such as a release-time task: open a GitHub issue for it and take
it off the list.

Work the stack from the top, one question at a time: ask the top question, say
what it blocks, and while it is open, talk about that question only. A question
that existing rules or code settle is not the person's decision and is not
recorded as one. An answer about a software plan also goes, numbered, in that
plan's Decisions section.

A finding that needs no decision becomes a queue item without being raised. A
finding that needs a decision goes on the stack and is mentioned in one line at
most. When it is unclear whether something needs the person, or the work is a
new tool or product rather than a fix, it goes on top of the stack.

Work on the builds tool goes first in the queue (add it with
`--priority first`): every agent works through builds, so what it lacks slows
every agent.

### Items and briefs

Each item's title is for the person to read. An agent's brief names the item ids
it takes, and its work lands (is rebased onto `main` and pushed) through
`builds land` with those ids. An item with an implementation brief names its
file, `plans/briefs/<area>.md` in the main checkout (git ignores `plans/`). The
coordinator runs `builds land` in the side worktree the branch's agent worked
in, once the agent is done there.

### Design-session items

The design session owns the boards (`design/mocks`), the design system
(`design/system`) and the canvas, the published page of all the boards. The
coordinator never picks up or dispatches a design-session item, and app work
that waits on a board waits until the board lands. How the design session draws,
records, reviews and lands a change is in `builds help workflow`, and how
boards, shots and the apps' views fit together is in
`projects/bae-rules/screen-content/build-each-screen-to-match-its-board.md`.

## Logs and known build failures

- The macOS app logs to the unified log under subsystem `fm.bae.desktop`, and
  Debug builds keep every level.

- xcodebuild fails with "Couldn't check out revision" or "file not found" on
  Sparkle when the SwiftPM cache is corrupt.

  - Sparkle is the SwiftPM package whose checkout corrupts most often.

  - Disconnect any VPN first: incomplete fetches corrupt the SwiftPM cache.

  - Then run:

    ```sh
    rm -rf bae-macos/bae/.build/runApp bae-macos/bae/.build/unsigned \
      bae-macos/bae/.build/SourcePackages
    rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/Sparkle*
    ./bae-macos/run.sh --no-open
    ```

- bae-audio finds FFmpeg through `FFMPEG_DIR`, and its tests find FFmpeg's
  libraries through `DYLD_LIBRARY_PATH`.

  - `.cargo/config.toml` sets both.
  - Don't export either: macOS drops `DYLD_*` whenever a shell script starts, so
    an export before `scripts/cargo.sh` never reaches the tests.

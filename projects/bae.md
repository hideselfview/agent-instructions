# bae

bae is a personal music library and player. A Rust core reads and writes the
library and does all the work. Native apps on each platform show the library
through a generated bridge. coven syncs the library between devices through S3,
Dropbox, OneDrive or iCloud.

This file holds the facts that every agent who works on bae needs. Rules for
specific files are separate (see "Making a change").

## Crates and apps

bae's Rust code is a workspace of crates:

- bae-app is the top crate. It uses bae-library and one crate per domain, such
  as bae-records and bae-import.
- bae-bridge gives the apps the core through uniffi.
  - Its `Bridge*` records and enums are defined in Rust, in
    `bae-bridge/src/types/`, and re-exported by `bae-bridge/src/types.rs`.
  - The build generates their Swift and Kotlin versions. Those are gitignored,
    so they are not in the repo or in review. To check a bridge type's fields,
    or whether an app type duplicates one, read `bae-bridge/src/types.rs`.

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
  Android are done: it is a launch shell with no screens. The shell has startup,
  single-instancing, crash capture, telemetry, the Velopack updater, release
  packaging, and one window that calls the bridge through the generated C#
  bindings.

  - Don't add Windows or Linux screens. Its desktop UI will be ported from the
    macOS app.
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

bae's `Cargo.toml` pins coven to one commit (`rev`). Move the pin as soon as
coven lands what bae needs. While coven lands a planned series of commits, move
the pin once, after the series.

### bae writes as if it were the only device

bae writes as though every device wrote to this one database, one write at a
time, in plain local SQL. Nothing in bae's schema, triggers or code is about
other devices, writes that arrive later, devices that were offline, or the order
in which remote rows apply. coven handles concurrent writes, rows whose parent
another device deleted, and conflicts. If a bae write cannot be written as
local, coven lacks something: fix coven, and never work around it in bae.

### Tables belong to their domain

Each table lives in the crate of the domain that owns it: records are in
bae-records and the import's tables in bae-import, for example. bae-store has
none. A domain's SQL files are in the same folder as a `.rs` file of the same
name, with their trigger tests in `<stem>_tests.rs`.

The schema is one migration, which bae-library assembles (`SCHEMA`) from each
crate's parts in crate order. A schema change edits its domain's SQL file and
never adds a second migration. No library has to advance from an older schema:
`scripts/reset-bae.py` resets development data.

Only a domain's own code writes its tables. The pre-commit hook runs coven's
structural checker (`scripts/owner-construction-check.sh`), and its
`database-boundary` rule refuses a crate that writes a table its `[database]`
rows in `owner-construction-check.toml` don't give it. Tests build library state
through the owning domain's writes, never by writing tables on a bare
connection. A named function in the owning domain plants the damage a test needs
(a value a CHECK refuses, a failing write), and a bare file open goes only
through `bae_store::test_fixtures::planted_damage::plant_in_library_file`.
bae-store gives no `Connection` to other crates: a domain's tests open their own
in-memory library with `Schema::open_bare_library(&conn)`.

A trigger never waits for a parent row. Foreign keys are satisfied for every
local write, and coven applies another device's rows in the order they were
written, parents first.

### Shapes change in one commit, with no compatibility code

When any of these shapes changes, edit the definition and update every caller in
the same commit:

- a table
- a bridge type
- an event payload
- an on-disk file layout
- the sync membership chain format
- an encryption scheme
- a cloud storage path

Don't add dual-shape flags, `#[serde(default)]` to absorb renames, or fallback
decoders for old data. Regenerate stale fixtures.

Schema and format changes, and the resets they need, are normal in development
for bae and coven alike. They are never a factor in a design decision: don't
prefer an option because it avoids one, and don't call one a cost or a caveat.
Never mention a reset to the person, even when a change that needs one lands.

### A screen shows its query's rows

Rust turns the rows of a screen's SQL query into bridge types, and the app draws
them, with no cache, enrichment or other step between the query and the screen.
The exceptions are values that change many times a second, which travel on keyed
latest-value streams, and state that exists only on screen (hover, focus, an
open popover or menu, a drag in progress), which the view keeps.

The full rules are
`projects/bae-rules/reactive-screens-read-plain-live-queries.md` and
`projects/bae-rules/high-frequency-values-skip-observed-ui-state.md`. They load
only when an agent edits matching files, so every plan, brief and review checks
a change's reads and fast values against them. A plan that proposes a shape they
forbid is wrong: raise it instead of building it.

## How work is tracked

Four kinds of session work on bae. The person decides what bae does. A
coordinator session plans the work, dispatches agents to do it, reviews what
they make and lands it. The agents each do one task in a side worktree. A design
session draws the screens as boards that the apps are then built to match.

### The decision stack and the work queue

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

The stack is ordered by how much each answer unblocks. Work it from the top, one
question at a time: ask the top question, say what it blocks, and while it is
open, talk about that question only. A question that existing rules or code
settle is not the person's decision and is not recorded as one. An answer about
a software plan also goes, numbered, in that plan's Decisions section.

A finding that needs no decision becomes a queue item without being raised. A
finding that needs a decision goes on the stack and is mentioned in one line at
most. When it is unclear whether something needs the person, or the work is a
new tool or product rather than a fix, it goes on top of the stack.

Work on the builds tool goes first in the queue (add it with
`--priority first`): every agent works through builds, so what it lacks slows
everything.

### Items and briefs

Each item's title is for the person to read. An agent's brief names the item ids
it takes, and its work lands (is rebased onto `main` and pushed) through
`builds land` with those ids. An item with an implementation brief points at it
in `plans/briefs/<area>.md`. `principles/coordinating-agents.md` says how the
coordinator plans, briefs, dispatches, reviews and lands. bae lands from its
landing worktree (side-7), and coven from side-1.

### Design-session items

The design session owns the boards (`design/mocks`), the design system
(`design/system`) and the canvas. The coordinator never picks up or dispatches a
design-session item, and app work that waits on a board waits until the board
lands. How the design session draws, records, reviews and lands a change is in
`principles/drawing-screens.md` and `principles/design-to-implementation.md`.

## Making a change

### The spec moves with every commit

`spec/bae.md` states what bae does. Every commit checks whether it changes or
adds anything the spec states or should state (behavior, a schema or sync shape,
a user-visible rule), and if it does, edits the spec in the same commit. A
commit without its spec edit is unfinished.

### Project rules

bae's rules for specific files are in `projects/bae-rules/`, each scoped to the
paths it covers. Claude Code receives them through `.claude/rules/` symlinks.
Codex receives the rule index in `AGENTS.md`: before editing files, list the
matching rules with
`~/.codex/agent-instructions/scripts/rules_review/matching_rules.py --project bae <path>...`
and read every listed file.

### File size

A source file must not exceed 1,000 lines (`scripts/check-source-file-size.py`,
with its frozen per-file exceptions). A Rust file over 1,000 lines keeps its
`#[cfg(test)]` module in a sibling `_tests.rs` file. Split production code along
its domain and ownership boundaries. Don't expose an owner's state or duplicate
types to make a split compile.

### Checking a change

Before you commit, run the pre-commit hook (the `.checks` lines the staged files
reach: formatters and static checks only) and the tests of the components the
change affects. Then commit and push. CI checks macOS, iOS, Android, Linux and
Windows. When a CI job fails, reproduce that job locally.

Test a crate with the features the hook builds it with, so the test reuses the
hook's build instead of compiling every crate again:
`scripts/cargo.sh test -p <crate> --features test-utils <filter>`, and
`--features oauth-providers,cloudkit,cast,test-keyring,dev-reset` for
bae-bridge. Use another feature set only to check that feature set.

Tests, benches and probes never write to the person's keychain: use an in-memory
keychain (`Coven::in_memory`) or a temporary one. This matters most for the
synced iCloud Keychain, where an entry reaches every device on the person's
Apple account (a probe library's restore code once appeared on their welcome
screen). Read the person's keychain only when they ask, for that task.

## Building and testing

The `builds` tool manages checkouts, build folders and app builds. `builds help`
and `principles/worktree-build-priming.md` explain how it works.

### Checkouts

- **`~/dev/bae` and `~/dev/coven` are the person's.** No agent edits, builds,
  commits or cherry-picks there, the coordinator included.
- **Side worktrees are for agents.** Work in the worktree your brief names, or
  get one with `builds new ~/dev/bae --for <who>`; never pick a side-N by hand.
  Whoever hands a worktree to an agent claims it
  (`builds claim <worktree> <who>`), keeps a one-line note on what the work is
  at (`builds note <worktree> <text>`), and releases it (`builds release`) when
  the work lands. `builds status` and the dashboard (http://127.0.0.1:7787/)
  show who has each one.

### Rust builds

Side worktrees build host Rust only: tests, clippy and the pre-commit hook. Run
every cargo command through `scripts/cargo.sh`, never bare `cargo`, and never
set `CARGO_TARGET_DIR`, `RUSTC_WRAPPER` or another `CARGO_*` variable on a
command: each build path is part of sccache's key, so an override gets no cache
hits.

`scripts/cargo.sh test`, `clippy`, `check`, `doc` and `nextest` run on this Mac
or the M1, whichever has more free cores. A test that needs this Mac's keychain,
a local server or a file outside the checkout fails on the M1: run it with
`BUILDS_CARGO_HOST=local`.

`builds` deletes build folders that no build used for days and keeps the rest
under their size caps. Leave the build folders of `~/dev/bae` and the app
checkouts alone.

### App builds

A change to Swift, Kotlin, C# or a bridge shape the apps compile against needs
an app build. Commit on your branch, then run this as a background task:

```sh
builds app-build bae <branch> [--test <Class>]... [--shots <prefix>]...
```

builds picks the steps from the paths the branch changes, then the lane (mac or
android) and a free app checkout, and builds there. A branch that changes only
Rust builds nothing. `--test <Class>` runs one test class, and
`--shots <prefix>` (`mac.` or `android.`) runs a shot selection.

Never run `macos-tests-run`, `macos-tests-all` or `android-tests` as a check:
they run every test and put windows on the person's screen, and CI runs the
suites. To build something else in an app checkout, add a `step` line and an
`on` line to `.app-build`.

The person builds their own test apps with
`builds build-app bae mac|android [<ref>]`. Notarized macOS releases come only
from `main`, and `scripts/release-macos.sh` builds them on CI.

### iOS simulators

Keep at most one iOS simulator runtime and one simulator device installed. When
you install a new runtime, delete the old runtime and its devices
(`xcrun simctl runtime list`, `xcrun simctl runtime delete`,
`xcrun simctl delete`). Each runtime takes 8 to 20 GB of the disk the builds
share.

## Logs and known build failures

- The macOS app logs to the unified log under subsystem `fm.bae.desktop`, and
  Debug builds keep every level:
  `/usr/bin/log show --last 10m --info --debug --predicate 'subsystem == "fm.bae.desktop"'`.

- When xcodebuild fails with "Couldn't check out revision" or "file not found"
  on Sparkle, the SwiftPM package that most often corrupts, disconnect any VPN
  (incomplete fetches corrupt the SwiftPM cache), then run:

  ```sh
  rm -rf bae-macos/bae/.build/runApp bae-macos/bae/.build/unsigned \
    bae-macos/bae/.build/SourcePackages
  rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/Sparkle*
  ./bae-macos/run.sh --no-open
  ```

- bae-audio finds FFmpeg through `FFMPEG_DIR`, and its tests find FFmpeg's
  libraries through `DYLD_LIBRARY_PATH`. `.cargo/config.toml` sets both. Don't
  export either: macOS drops `DYLD_*` whenever a shell script starts, so an
  export before `scripts/cargo.sh` never reaches the tests.

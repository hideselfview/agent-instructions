# bae

Personal music library + playback. Rust core (bae-app over bae-library and the
domain crates), platform UIs via uniffi (bae-bridge → bae-macos, bae-ios,
bae-android), and cloud sync via coven supporting S3 / Dropbox / OneDrive /
iCloud.

bae-avalonia is a Windows/Linux infrastructure skeleton, not a UI: startup,
single-instancing, crash capture, telemetry, the Velopack updater and release
packaging, and one window that calls the bridge through the uniffi-generated C#
bindings. It has no views, stores, or strings to update when a bridge shape
changes; it only has to keep compiling against the regenerated bindings and pass
its smoke test (`bae-avalonia/smoke-test.sh` on macOS, and the Windows and Linux
CI jobs). The desktop UI will be ported from the macOS app later.

bae-ios is paused until macOS and Android are done: it is a launch shell with no
screens. Don't build the iOS app or the iOS bridge locally, don't compile BaeKit
for iOS to check a change, don't add iOS UI or iOS variants of macOS views, and
don't keep iOS build output warm. CI's iOS jobs still run; when one goes red,
fix it then. Work that would touch iOS stops at macOS and Android.

Keep at most one iOS simulator runtime and one simulator device installed: when
a new runtime is installed, delete the old runtime and its devices
(`xcrun simctl runtime list`, `xcrun simctl runtime delete`,
`xcrun simctl delete`). Each runtime takes 8–20 GB of the disk the builds share.

Tests, benches and probes never write to the person's keychain. Use an in-memory
keychain (`Coven::in_memory`) or a temporary one. That goes double for the
synced iCloud Keychain: an entry there reaches every device on the person's
Apple account, and a probe library's restore code once showed up on their
welcome screen. Read the person's keychain only when they ask for it, for that
task.

bae writes as though every device wrote to this one database, one write at a
time: plain local SQL, with nothing in its schema, triggers or code about other
devices, writes arriving later, devices apart or the order remote rows apply.
Concurrent writes, deleted parents and conflicts are coven's: it merges, takes
rows out and keeps lost values. If a bae write cannot be written as local, coven
is missing something: fix coven, never work around it in bae (the person,
2026-11-13: "the rule of coven is you write as local; if you can't do that,
something is not right about coven").

Move bae's coven pin as soon as coven has landed what bae needs, even when the
new coven needs the library recreated. Never hold a bump back to save them a
reset. While coven is landing a planned series of commits, move the pin once,
after the series finishes, not after each commit.

Schema and format changes, and the resets they need, are normal in development,
for bae and coven alike. They are never a factor in a design decision: don't
prefer an option because it avoids one, and don't call one out as a cost or a
caveat when describing, weighing or recommending work. Never mention a reset to
the person, not even when a change that needs one lands: they assume things
changed, and ask when they want to know.

The macOS app logs to the unified log under subsystem `fm.bae.desktop`, every
level kept in Debug builds:
`/usr/bin/log show --last 10m --info --debug --predicate 'subsystem == "fm.bae.desktop"'`.

bae and coven keep two lists in the `builds` store, never in a file or a commit
(`builds help lists` has the commands): the **decision stack**, what waits on
the person (questions, and work only they can do, or work held until they start
it), and the **work queue**, the task list for the coordinator and its agents:
only what they can do. Nothing to do lives anywhere else (not in an agent's
brief, a handoff, a plan, the decision log or a chat message) unless an item
points at it. Commits, code and repo files never name list ids or the lists.

The stack is ordered by how much each answer unblocks (the person, 2026-11-09:
"prioritize questions that unblock the most stuff"). Work it from the top, one
question at a time: ask the top one and say what it blocks. While a question is
open, talk about that question only. A finding that needs no decision is an item
without being raised; one that needs a decision goes on the stack, and is
mentioned in one line at most. When it is unclear whether something needs the
person, or the work is large in scope (a new tool or product, not a fix), it
goes on top. An answer about a software plan also goes, numbered, in that plan's
Decisions section. A question settled from existing rules or code is not their
decision and is not recorded as one.

Work on the builds tool goes first in the queue, ahead of every other item:
every agent works through it, so what it lacks or does wrong slows everything
(the person, 2026-11-13: "anything builds related becomes TOP PRIORITY"). An
item for the builds tool is added with `--priority first`.

Neither list holds what is not relevant now (a release-time task, something held
for a later stage): open a GitHub issue for it and take it off the list (the
person, 2026-11-13: "if there are TODOs that are NOT relevant right now just
make a GH for each one and REMOVE THEM").

An item for the design session (its boards, `design/mocks`, `design/system`, the
canvas) is the design session's: the coordinator never picks it up or dispatches
it, and app work that waits on its board waits until the board lands (the
person, 2026-11-13).

The design session records each design change when it draws the boards
(`builds design record`): what changed, each board's canvas link, how its review
page reads (headings, words, and the boards under each, which the dashboard
draws live from the session's worktree), and for every board, component, shared
design-system source and spec section the change touches, the item that builds
it in the apps or how it was checked that the app already matches. The person
reviews it in the dashboard sidebar's Waiting on you tab, where its Details
opens that review page, which replaces the page the session kept by hand; the
session keeps no page of its own. `builds design render DIR BOARD` prints a
board as a page of HTML to look at while drawing. Once the person says to land
it, the session approves it (`builds design approve N --by-person`) and lands it
with `land.sh --design N` from bae's landing worktree (side-7). For the design
session, "land" means committing the boards, design system and spec lines to git
and pushing them to `main`; it closes the change, which takes it off the tab and
frees the app work that waits on it. The apps being built to match is the items'
job, and recording a change puts every piece of app work it needs on the queue
in the same step (the person, 2026-11-10 and 2026-11-13).

Each item's title is for the person (the person, 2026-11-13: "this will be the
human title"). An agent's brief names the item ids it takes, and its work lands
through `land.sh` with those ids. An item with an implementation brief points at
it in `plans/briefs/<area>.md`. How the coordinator plans, briefs, dispatches
(Sonnet executes), reviews and lands
(`~/dev/agent-instructions/scripts/land/land.sh` from bae side-7 and coven
side-1) is `principles/coordinating-agents.md`.

Project rules are path-scoped atomic files in `projects/bae-rules/`. Claude Code
receives them through `.claude/rules/` symlinks. Codex receives the rule index
in `AGENTS.md`; before editing files in this project, list matching full rule
bodies with
`~/.codex/agent-instructions/scripts/rules_review/matching_rules.py --project bae <path>...`
and read every listed file. This file holds the always-on project facts.

## Local verification

Normal changes use the pre-commit hook (the `.checks` lines the staged files
reach: formatters and static checks only) plus tests for the components they
affect. Commit and push after those checks pass; CI validates macOS, iOS,
Android, Linux, and Windows concurrently. Reproduce an individual CI gate
locally when it fails.

Test a crate with the feature set the hook builds it with, so the test reuses
the hook's build instead of making another copy of every crate:
`scripts/cargo.sh test -p <crate> --features test-utils <filter>`, and
`--features oauth-providers,cloudkit,cast,test-keyring,dev-reset` for
bae-bridge. Another feature set is another copy; use one only to check that
feature set itself.

`scripts/check.sh` serializes every non-Windows CI gate on one machine. Run it
only when explicitly validating the complete non-Windows system locally, not as
the routine local gate before a commit or push.

## The spec moves with every commit

`spec/bae.md` states what bae does. Every commit checks whether it changes, or
adds, anything the spec states or should state — behavior, a schema or sync
shape, a user-visible rule — and edits the spec in that same commit. A commit
without its spec edit is unfinished: never leave the spec for a follow-up.

## File layout

Source files may not exceed 1,000 lines (`scripts/check-source-file-size.py`,
with its frozen per-file exceptions). A Rust file over 1,000 lines must keep its
`#[cfg(test)]` module in a sibling `_tests.rs` file. Split production code along
its existing domain and ownership boundaries; do not expose owner state or
duplicate types to make a split compile.

## Reads and fast values

Everything a screen shows comes from SQL, except values that change many times a
second, which travel on keyed latest-value streams, and state that lives only on
screen (hover, focus, an open popover or menu, a drag in progress), which the
view holds and SQL never sees.

A read that must follow changes (a reactive screen, or a subscriber in the
bridge, automation or MCP) is a plain coven live query; any other read is a
plain SQL read. SQLite does the filtering, sorting, joining, grouping, counting
and windowing, and everything a screen shows beside its rows (counts, totals,
groups and section headers, "N of M", sums, facets) comes from SQL, in the same
query or its own live query. SQLite is local, so the query is the plain one
(GROUP BY, COUNT, JOIN straight from the domain's tables); a table of pre-added
counts kept by triggers comes only after the plain query measures too slow.
Nothing is kept in memory between reads, no side pass, no forced rerun, no size
cutoff between read paths. A long list pages by cursor (its request carries the
sort values of the row at the screen's edge, sought through the order's index)
and jumps by offset, with an index for the jump only when it measures slow
enough to warrant one. The full rule is
`projects/bae-rules/reactive-screens-read-plain-live-queries.md`.

A value that changes many times a second (playback position, transfer bytes,
rates, levels) travels on a keyed latest-value stream and is drawn by a native
view that reads the latest value per key once per display frame, never through
SwiftUI, Compose or Avalonia observed state and never with a main-thread hop per
change. The full rule is
`projects/bae-rules/high-frequency-values-skip-observed-ui-state.md`.

Every plan, brief and review checks a change's reads and fast values against
these two rules; a plan that proposes a forbidden shape is wrong and is raised,
not built.

## Schema and compatibility

Tables live with the domain that owns them, in that domain's crate: records in
bae-records, members and people in bae-people, the import's in bae-import, the
library views' in bae-views, and so on; bae-store holds none. The schema is one
migration, assembled by bae-library (`SCHEMA`) from each crate's parts in crate
order. A domain's SQL files sit in its home beside a same-stem `.rs`, with their
trigger tests in `<stem>_tests.rs`. A schema change edits its domain's SQL file;
it never adds a second migration. No library has to advance from an older
schema: development data is reset with `scripts/reset-bae.py`.

Only a domain's homes write its tables, as the `[database]` rows of
`owner-construction-check.toml` list them; the checker's `database-boundary`
rule holds every crate to that. Tests build library state through the owning
domain's own writes, never by writing tables on a bare connection. Damage a test
needs (a value a CHECK refuses, a failing write) is planted by a named function
in the owning domain, and a bare file open goes only through
`bae_store::test_fixtures::planted_damage::plant_in_library_file`. bae-store
hands out no `Connection`: a home's tests open their own in-memory library from
`Schema::bare_library_parts()`. A trigger never waits for a parent row: foreign
keys hold for every local write, and coven applies another device's rows in the
order of their writes, parents first.

When another canonical shape changes — bae-bridge types, UiEventBus event
payloads, on-disk file layouts, sync membership chain format, encryption
schemes, cloud storage paths — edit the definition and update every caller in
one PR. Do not add dual-shape compatibility flags, `#[serde(default]` to
silently absorb renames, or fallback decoders for old data. Regenerate stale
fixtures.

## Bridge types are defined in Rust, generated per language

The `Bridge*` records/enums live in `bae-bridge/src/types/` and are re-exported
by `bae-bridge/src/types.rs` (`uniffi::Record` / `uniffi::Enum`). The
Swift/Kotlin equivalents are generated at build and gitignored
(`bae-macos/bae/bae/bae_bridge.swift`, `bae-bridge/swift-bindings/`) — absent
from the repo and from review. To check a bridge type's existence/fields, or
whether a UI type duplicates one, read `bae-bridge/src/types.rs`; never conclude
from the (missing) generated file.

## Worktrees and build output

Each checkout builds into its own folders on the build volume:
`/Volumes/Builds/bae/<checkout>/` (`main`, `side-1`, …), linked from the paths
`.build-dirs` lists (`target`, `target-macos`, `target-ios`, `target-android`,
the Xcode `.build` dirs, `BaeKit/Frameworks`, `bae-android/debug-symbols`,
Gradle's, Avalonia's `obj`/`bin`). The `builds` tool manages them; the layout,
the app checkouts' locks, the measurements behind them, sccache and SwiftPM
recovery are in `principles/worktree-build-priming.md`. The bae specifics:

- **Side worktrees build host Rust only** (`target`: tests, clippy, the
  pre-commit hook). App builds — the bridge for macOS, iOS or Android, the Xcode
  apps, BaeKit, Gradle, Avalonia — stay warm in main and in the app checkouts of
  each lane, the mac lane (Xcode, `target-macos`) and the android lane (Gradle,
  `target-android`), which share no build output and so each have checkouts,
  locks and a queue of their own:

  - **main**, the person's checkout. Notarized macOS releases come only from
    main (`scripts/release-macos.sh` runs the `release-macos.yml` workflow on
    it, so they build on CI, not here). The person builds their test apps with
    `builds build-app bae mac|android [<ref>]`, in that lane's app checkout on
    its lock: `mac` copies the app to `~/Applications/bae-test/bae.app`,
    `android` installs it on the connected phone (`bae-android/run.sh`). Neither
    opens the app.

  - **The app checkouts**, for every agent whose change needs an app build
    (Swift, Kotlin, C#, or a bridge shape the apps compile against):
    `~/dev/bae-worktrees/side-1` is the mac lane's (recorded for the tool with
    `builds app-checkout mac ~/dev/bae-worktrees/side-1`), and the android
    lane's are other side worktrees recorded with
    `builds app-checkout android <worktree>` (a lane may have several, each with
    a lock of its own; `builds app-checkout remove` takes one out);
    `builds status` shows which. Commit on your branch in your own worktree,
    then run, as a background task:

    ```sh
    builds app-build bae <branch> [<step>...] [--test mac|android <Class>]... [--shots mac|android <prefix>]...
    ```

    It takes the queue for the locks of each lane the steps are in
    (`/Volumes/Builds/bae/app.locks/<checkout>` for the mac lane,
    `app-android.locks/<checkout>` for the android; the next in line takes
    whichever checkout of the lane is free), checks the branch out in that app
    checkout (which must be clean) and runs the lane's steps of the repo's
    `.app-build`, in the order named, stopping at the first that fails; with no
    step named it runs every one. The lanes build at the same time, each output
    line behind `[mac side-N]` or `[android side-N]` (the checkout it builds
    in), and the command fails if either lane does, saying which; a mac-only
    build and an android-only build don't wait on each other.
    `builds app-build bae --list` prints the steps with their lanes: `macos`
    (build the app), `macos-tests` (compile its unit tests), `macos-tests-run`
    (run them, without rebuilding; the UI tests are not among them),
    `macos-tests-all` (build and run the unit and UI tests, signing the UI test
    runner so Gatekeeper lets it launch) in the mac lane; `android-bridge` and
    `android` (the bridge, then the Kotlin and both test targets),
    `android-tests` (run the unit tests from that output) in the android lane. A
    change to a bridge shape or to Swift code runs `macos` and `macos-tests`;
    one to Kotlin runs `android-bridge` and `android`; a bridge shape runs both
    sets, in one command. Then only the test class that proves the change, with
    `--test mac <TestClass>` or `--test android <TestClass>` after the steps
    (`.app-build` declares once per lane how a lane runs one class, the `test`
    lines with `{class}` in them; no step is added per class, and with no step
    named only the classes run). A screenshot area is picked the same way, with
    `--shots mac <prefix>` or `--shots android <prefix>` (any number, the
    `shots` lines with `{select}`; `mac-shots` and `android-shots` stay for a
    whole platform, and `mac-window-shots` for the shots shown in their windows,
    which bring the app to the front). Never `macos-tests-run`,
    `macos-tests-all` or `android-tests` as a check, since they run every test
    and put windows on the person's screen. CI runs the suites. A step is a
    program, not a shell string: one that needs a shell is a script in the repo
    (`bae-android/compile-all.sh`). A step that runs past its time limit (45
    minutes, or its `limit <step> <time>` line in `.app-build`) is killed and
    fails the build, which gives the lock up. To build something else there
    (`bae-ios/run.sh`, `bae-avalonia/smoke-test.sh`, a screenshot script), add a
    `step <lane> <name> <command>` line.

    Cargo builds there, as in every side worktree, run without incremental
    compilation (`builds cargo` passes `--config build.incremental=false`
    everywhere but main), so sccache caches every crate a branch built, and its
    folders take the lower `app` caps in `.build-dirs`. Run every cargo build
    through `scripts/cargo.sh` so the setting reaches it.

  Another warm app build is fine while a wave of work needs it, such as when the
  person asks for one in a particular worktree; clear it once that work has
  moved on.

- **The main checkouts (`~/dev/bae`, `~/dev/coven`) are the person's.** No agent
  edits, builds, commits or cherry-picks there, the coordinator included. The
  coordinator lands commits from a side worktree claimed for landing: rebase or
  cherry-pick onto `origin/main` there and push to `main`.

- **Claims:** work in the worktree your brief names, or get a new one with
  `builds new ~/dev/bae --for <who>`; never pick a side-N by hand. Whoever hands
  a worktree out claims it (`builds claim <worktree> <who>`) and releases it
  (`builds release`) when the work lands, and keeps a one-line note on it of
  what the work is at (`builds note <worktree> <text>`), updated when the agent
  starts, hands off or has a commit landed. `builds status` shows who holds each
  one and the note; `builds dashboard` shows the same, with each worktree's
  commits waiting to land, at http://127.0.0.1:7787/.

- **New side worktree:** `builds new ~/dev/bae` hands out a free
  `~/dev/bae-worktrees/side-N` (no claim, clean, every commit on `origin/main`
  by patch) checked out afresh from `origin/main`, or creates the next one when
  none is free, and runs its `post-checkout`, which links the build folders,
  `CLAUDE.md`, the main checkout's `bae-ffmpeg`, the signing/feature xcconfigs
  and `bae-android/local.properties`, and seeds the macOS app's
  `Package.resolved` from main. It then fills the worktree's host `target`
  (never an app lane's folders) with a copy-on-write clone of a warm checkout's,
  when that folder is empty, so the first clippy or hook run there rebuilds no
  third-party build script. It compiles nothing. `builds retire <worktree>`
  removes a side worktree and its folders once its work has moved on;
  `builds prune` retires a free one nothing has used for a day, never an app
  checkout or one with unlanded commits.

  bae-audio links FFmpeg and finds it via `FFMPEG_DIR` (set to `bae-ffmpeg/dist`
  in `.cargo/config.toml`), so bae-audio and every crate above it build in any
  checkout. Their tests find FFmpeg's dylibs through `DYLD_LIBRARY_PATH` in the
  same config, which cargo passes to every process it runs, so no export is
  needed. macOS drops `DYLD_*` whenever a shell script starts, so an export
  before `scripts/cargo.sh` would never reach the tests anyway.

- **rust-analyzer builds into `target-ra`.** The Claude Code session runs it
  through agent-instructions' `rust-analyzer` plugin
  (`claude/plugins/rust-analyzer`), which starts it with
  `cargo.targetDir = "target-ra"` and `checkOnSave = false`. It still runs the
  build scripts and proc macros navigation needs, a `cargo check --workspace`
  into `target-ra`; it no longer checks the workspace into main's `target` on
  every landing. Its check-on-save is off: the pre-commit hook and clippy catch
  compile errors.

- **Don't set `CARGO_TARGET_DIR`, `RUSTC_WRAPPER` or other `CARGO_*` variables
  per command.** Host cargo (tests, clippy, the pre-commit hook) builds into the
  checkout's `target`; `bae-bridge/build-macos.sh`, `build-ios.sh` and
  `build-android.sh` build into `target-macos`, `target-ios` and
  `target-android`, each its own folder because they read the built staticlib
  back to generate bindings. Each of those paths is part of sccache's key, so an
  inline override gets no cache hits. The one wrapper set in the repo is
  `scripts/clippy-crates.sh`'s, `scripts/clippy-rustc-wrapper.sh`, which itself
  runs sccache: it lints only the crates named with `-p` and compiles the
  workspace crates below them through sccache, which cannot cache a clippy run.

- **Build copies no build used for three days are deleted, and a copy a newer
  copy built from other sources replaced at once**, at the start of the next
  hook or build script in that checkout and as each build through
  `scripts/cargo.sh` exits, never while a build in that checkout runs; the hooks
  and build scripts run cargo through `scripts/cargo.sh`, which records each
  copy cargo uses and holds the checkout's guard while cargo runs. Run cargo
  through it in a new script too. **Incremental folders nothing wrote to for six
  hours are deleted** at the same moment, in every checkout including main's,
  and every incremental folder in the other checkouts, which build without them;
  a build holding the target keeps them, and losing one costs an incremental
  rebuild of its crate. Under the free-space floor, incremental folders nothing
  wrote to for an hour go first, in every checkout, claimed ones and main's and
  the app checkouts' included, before any whole folder. **Folders are deleted
  whole when they still pass their cap** in `.build-dirs`, and refilled by that
  build. Leave main's and the app checkouts' own lane's otherwise: the person
  installs from main and tests from the app checkouts. The free-space floor
  never deletes theirs whole. `builds status` shows each checkout's sizes.

- **Sparkle is the SwiftPM dep that corrupts** (xcodebuild fails with "Couldn't
  check out revision" or "file not found" on Sparkle). Every xcodebuild of the
  macOS app resolves its packages into `bae-macos/bae/.build/SourcePackages`:

  ```sh
  rm -rf bae-macos/bae/.build/runApp bae-macos/bae/.build/unsigned \
    bae-macos/bae/.build/SourcePackages
  rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/Sparkle*
  ./bae-macos/run.sh --no-open
  ```

  The two trees and the packages' folder are defined in
  `scripts/xcode-trees.sh`; `builds` deletes a tree no build used for three
  days.

  Disconnect VPN first — incomplete fetches corrupt the SwiftPM cache.

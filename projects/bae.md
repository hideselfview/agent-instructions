# bae

Personal music library + playback. Rust core (bae-core), platform UIs via uniffi
(bae-bridge → bae-macos, bae-ios, bae-android), and cloud sync via coven
supporting S3 / Google Drive / Dropbox / OneDrive / iCloud.

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

Tests, benches and probes never write to the person's keychain. Use an in-memory
keychain (`coven::TestCoven`) or a temporary one. That goes double for the
synced iCloud Keychain: an entry there reaches every device on the person's
Apple account, and a probe library's restore code once showed up on their
welcome screen. Read the person's keychain only when they ask for it, for that
task.

Never read, print or edit `~/.config/bae-dev/profile.toml`: it holds the
person's storage and service credentials. Never launch the app with
`--dev-reset`; the person runs `scripts/reset-bae.py` themselves.

Project rules are path-scoped atomic files in `projects/bae-rules/`. Claude Code
receives them through `.claude/rules/` symlinks. Codex receives the rule index
in `AGENTS.md`; before editing files in this project, list matching full rule
bodies with
`~/.codex/agent-instructions/scripts/rules_review/matching_rules.py --project bae <path>...`
and read every listed file. This file holds the always-on project facts.

## Local verification

Normal changes use the dependency-aware pre-commit hook plus tests for the
components they affect. Commit and push after those checks pass; CI validates
macOS, iOS, Android, Linux, and Windows concurrently. Reproduce an individual CI
gate locally when it fails.

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

## Schema and compatibility

The database schema is one flat migration,
`bae-core/migrations/001_initial.sql`. A schema change edits that file; it never
adds a second migration. No library has to advance from an older schema:
development data is reset with `rm -rf ~/.bae`.

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
the Xcode `.build` dirs, Gradle's, Avalonia's `obj`/`bin`). The `builds` tool
manages them; the layout, the app checkout's lock, the measurements behind it,
sccache and SwiftPM recovery are in `principles/worktree-build-priming.md`. The
bae specifics:

- **Side worktrees build host Rust only** (`target`: tests, clippy, the
  pre-commit hook). App builds — the bridge for macOS, iOS or Android, the Xcode
  apps, BaeKit, Gradle, Avalonia — stay warm in two checkouts:

  - **main**, for the person's installs. Notarized macOS releases come only from
    main (`scripts/release-macos.sh` runs the `release-macos.yml` workflow on
    it).

  - **`~/dev/bae-worktrees/side-1`, the app checkout** (recorded for the tool
    with `builds app-checkout ~/dev/bae-worktrees/side-1`), for every agent
    whose change needs an app build (Swift, Kotlin, C#, or a bridge shape the
    apps compile against). Take it under `/Volumes/Builds/bae/app.lock`: commit
    on your branch in your own worktree, then, as a background task:

    ```sh
    lockf -k /Volumes/Builds/bae/app.lock sh -c '
      cd ~/dev/bae-worktrees/side-1 &&
      { test -z "$(git status --porcelain)" || { git status --short; exit 1; }; } &&
      git checkout -q --detach <branch> &&
      ./bae-macos/run.sh --no-open'
    ```

    with the build you need in place of `bae-macos/run.sh` (`bae-ios/run.sh`,
    `bae-android/run.sh`, `bae-avalonia/smoke-test.sh`, a screenshot script,
    tests), chained in the same command. The person tests an agent's app build
    there.

  Another warm app build is fine while a wave of work needs it, such as when the
  person asks for one in a particular worktree; clear it once that work has
  moved on.

- **New side worktree:** `builds new ~/dev/bae` creates the next free
  `~/dev/bae-worktrees/side-N` from `origin/main` and runs its `post-checkout`,
  which links the build folders, `CLAUDE.md`, the main checkout's `bae-ffmpeg`,
  the signing/feature xcconfigs and `bae-android/local.properties`, and seeds
  the macOS app's `Package.resolved` from main. It builds nothing.
  `builds retire <worktree>` removes a side worktree and its folders once its
  work has moved on.

  bae-audio links FFmpeg and finds it via `FFMPEG_DIR` (set to `bae-ffmpeg/dist`
  in `.cargo/config.toml`), so bae-audio and bae-core build in any checkout.
  Running their tests needs the FFmpeg dylibs on the loader path:
  `DYLD_LIBRARY_PATH=$PWD/bae-ffmpeg/dist/lib` (or the export
  `scripts/setup-ffmpeg.sh` prints).

- **Don't set `CARGO_TARGET_DIR`, `RUSTC_WRAPPER` or other `CARGO_*` variables
  per command.** Host cargo (tests, clippy, the pre-commit hook) builds into the
  checkout's `target`; `bae-bridge/build-macos.sh`, `build-ios.sh` and
  `build-android.sh` build into `target-macos`, `target-ios` and
  `target-android`, each its own folder because they read the built staticlib
  back to generate bindings. Each of those paths is part of sccache's key, so an
  inline override gets no cache hits.

- **Build copies no build used for three days are deleted**, at the start of the
  next hook or build script in that checkout; the hooks and build scripts run
  cargo through `scripts/cargo.sh`, which records each copy cargo uses. Run
  cargo through it in a new script too. **Folders are deleted whole when they
  still pass their cap** in `.build-dirs`, and refilled by that build. Leave
  main's and the app checkout's otherwise: the person installs from main and
  tests from the app checkout. The free-space floor never deletes theirs.
  `builds status` shows each checkout's sizes.

- **Sparkle is the SwiftPM dep that corrupts** (xcodebuild fails with "Couldn't
  check out revision" or "file not found" on Sparkle):

  ```sh
  rm -rf bae-macos/bae/.build/derivedData
  rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/Sparkle*
  cd bae-macos/bae && xcodebuild -project bae.xcodeproj -scheme bae \
    -derivedDataPath .build/derivedData build
  ```

  Disconnect VPN first — incomplete fetches corrupt the SwiftPM cache.

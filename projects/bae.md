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
manages them; the layout, the measurements behind it, sccache and SwiftPM
recovery are in `principles/worktree-build-priming.md`. The bae specifics:

- **New side worktree:** `builds new ~/dev/bae` creates the next free
  `~/dev/bae-worktrees/side-N` from `origin/main` and runs its `post-checkout`,
  which links the build folders, `CLAUDE.md`, the main checkout's `bae-ffmpeg`
  and the signing/feature xcconfigs, then primes: builds the bridge and the
  macOS app once so the first app build is incremental. For work that never
  builds an app (bae-core, docs), skip the prime with
  `builds new --no-prime ~/dev/bae`. `builds retire <worktree>` removes a side
  worktree and its folders once its branch has landed.

  bae-core finds FFmpeg via `FFMPEG_DIR` (set to `bae-ffmpeg/dist` in
  `.cargo/config.toml`), so it builds with no prime. Running the tests needs the
  FFmpeg dylibs on the loader path: `DYLD_LIBRARY_PATH=$PWD/bae-ffmpeg/dist/lib`
  (or the export `scripts/setup-ffmpeg.sh` prints).

- **Don't set `CARGO_TARGET_DIR`, `RUSTC_WRAPPER` or other `CARGO_*` variables
  per command.** Host cargo (tests, clippy, the pre-commit hook) builds into the
  checkout's `target`; `bae-bridge/build-macos.sh`, `build-ios.sh` and
  `build-android.sh` build into `target-macos`, `target-ios` and
  `target-android`, each its own folder because they read the built staticlib
  back to generate bindings. Each of those paths is part of sccache's key, so an
  inline override gets no cache hits.

- **Folders are deleted whole when they pass their cap** in `.build-dirs`, at
  the start of the next hook or build script in that checkout, and refilled by
  that build. Leave them otherwise: the person tests from a side worktree and
  reuses its build. `builds status` shows each checkout's sizes.

- **Sparkle is the SwiftPM dep that corrupts** (xcodebuild fails with "Couldn't
  check out revision" or "file not found" on Sparkle):

  ```sh
  rm -rf bae-macos/bae/.build/derivedData
  rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/Sparkle*
  cd bae-macos/bae && xcodebuild -project bae.xcodeproj -scheme bae \
    -derivedDataPath .build/derivedData build
  ```

  Disconnect VPN first — incomplete fetches corrupt the SwiftPM cache.

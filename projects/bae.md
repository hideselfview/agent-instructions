# bae

Personal music library + playback. Rust core (bae-core), platform UIs via uniffi
(bae-bridge → bae-macos, bae-ios, bae-android) plus Windows and Linux
(bae-avalonia, consuming uniffi-generated C# bindings), and cloud sync via coven
supporting S3 / Google Drive / Dropbox / OneDrive / iCloud.

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

Source files may not exceed 1,500 lines. A Rust file over 1,000 lines must keep
its `#[cfg(test)]` module in a sibling `_tests.rs` file. Split production code
along its existing domain and ownership boundaries; do not expose owner state or
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

## Worktrees and build priming (bae-macos)

The `post-checkout` hook (`scripts/hooks/post-checkout`, installed via
`scripts/install-hooks.sh`) primes the macOS SwiftPM/xcodebuild cache on
new-worktree creation, with the same flags the pre-commit hook uses
(`-scheme bae`, `-derivedDataPath .build/derivedData`). The general mechanics —
skipping this prime, the sccache temp-dir pitfalls, and SwiftPM cache recovery —
are in `principles/worktree-build-priming.md`. The bae specifics:

- **Lean worktree for bae-core-only changes** (nothing under bae-bridge or the
  platform apps): the macOS prime is redundant — the pre-commit hook skips the
  macOS build when no bridge/macOS files changed. Create without the prime and
  symlink the two things bae-core needs:

  ```sh
  git -c core.hooksPath=/dev/null worktree add -b <branch> <path> origin/main
  ln -sfn /path/to/main/bae-ffmpeg <path>/bae-ffmpeg   # prebuilt FFmpeg dist
  ln -sf ~/dev/agent-instructions/projects/bae.md <path>/CLAUDE.md
  ```

  bae-core finds FFmpeg via `FFMPEG_DIR` (set to `bae-ffmpeg/dist` in
  `.cargo/config.toml`), so it builds with no prime — the MusicBrainz disc ID is
  pure Rust now, no native libdiscid to link. Running the tests needs the FFmpeg
  dylibs on the loader path: `DYLD_LIBRARY_PATH=$PWD/bae-ffmpeg/dist/lib` (or
  the export `scripts/setup-ffmpeg.sh` prints).

  **Don't set `CARGO_TARGET_DIR` or `RUSTC_WRAPPER` per command.** The shell
  environment points every checkout at one shared target dir with sccache on;
  overriding either inline gives that worktree its own cold dir and no cache,
  which is both slower and the thing that fills the disk when several agents
  build at once. The pre-commit hook reads the shared value (it defaults its own
  only when nothing is set), so a plain `git commit` is correct. Earlier
  guidance here said the opposite — it was written when each worktree had its
  own fresh target dir, which is exactly the condition that broke sccache.

  One exception the shared dir does not cover: `bae-bridge/build-macos.sh`,
  `build-ios.sh`, and `build-android.sh` each pin their own per-checkout target
  dir (`target-macos/`, `target-ios/`, `target-android/`) on purpose, because
  they read the built staticlib back out to generate bindings and a shared dir
  could be rewritten by a concurrent build between the build and that read. The
  pre-commit hook does not run them (it runs fmt, clippy on the changed crates,
  linters, and the catalog checks); the worktree prime does (`post-checkout`, or
  `build-macos.sh` by hand before a macOS test run), and each run leaves a ~16
  GB `target-macos/` in that worktree. Leave it there after the branch lands:
  the person tests from that worktree and reuses the build. The disk sweep
  removes it once idle; delete it by hand only when the disk is actually full
  and it is the idle candidate.

- **Sparkle is the SwiftPM dep that corrupts** (pre-commit fails with "Couldn't
  check out revision" or "file not found" on Sparkle):

  ```sh
  rm -rf bae-macos/bae/.build/derivedData
  rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/Sparkle*
  cd bae-macos/bae && xcodebuild -project bae.xcodeproj -scheme bae \
    -derivedDataPath .build/derivedData build
  ```

  Disconnect VPN first — incomplete fetches corrupt the SwiftPM cache.

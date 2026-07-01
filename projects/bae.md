# bae

Personal music library + playback. Rust core (bae-core), platform UIs via uniffi
(bae-bridge → bae-macos, bae-ios, bae-android) plus Windows (bae-windows +
bae-windows-ffi), and cloud sync via coven supporting S3 / Google Drive /
Dropbox / OneDrive / iCloud.

Project rules are path-scoped atomic files in `projects/bae-rules/`. Claude Code
receives them through `.claude/rules/` symlinks. Codex receives the rule index
in `AGENTS.md`; before editing files in this project, list matching full rule
bodies with
`~/.codex/agent-instructions/scripts/rules_review/matching_rules.py --project bae <path>...`
and read every listed file. This file holds the always-on project facts.

## Greenfield — break things and move on

Pre-1.0. `rm -rf ~/.bae` is the migration strategy. When the canonical shape of
anything changes — DB schema, bae-bridge types, UiEventBus event payloads,
on-disk file layouts, sync membership chain format, encryption schemes, cloud
storage paths — edit the definition and update every caller in one PR. No
migration shims, no dual-shape compatibility flags, no `#[serde(default)]` to
silently absorb renames, no fallback decoders for old data. If a fixture is
stale, regenerate it.

## Bridge types are defined in Rust, generated per language

The `Bridge*` records/enums live in `bae-bridge/src/types.rs` (`uniffi::Record`
/ `uniffi::Enum`). The Swift/Kotlin equivalents are generated at build and
gitignored (`bae-macos/bae/bae/bae_bridge.swift`, `bae-bridge/swift-bindings/`)
— absent from the repo and from review. To check a bridge type's
existence/fields, or whether a UI type duplicates one, read
`bae-bridge/src/types.rs`; never conclude from the (missing) generated file.

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
  `.cargo/config.toml`) and libdiscid via the brew `LIBRARY_PATH` there, so it
  builds with no prime. Running the tests needs the FFmpeg dylibs on the loader
  path: `DYLD_LIBRARY_PATH=$PWD/bae-ffmpeg/dist/lib` (or the export
  `scripts/setup-ffmpeg.sh` prints). Build/test/commit with
  `CARGO_TARGET_DIR=target-iso RUSTC_WRAPPER=` (one warm dir, sccache off) —
  inline on `git commit` too, so the pre-commit hook reuses the dir.

- **Sparkle is the SwiftPM dep that corrupts** (pre-commit fails with "Couldn't
  check out revision" or "file not found" on Sparkle):

  ```sh
  rm -rf bae-macos/bae/.build/derivedData
  rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/Sparkle*
  cd bae-macos/bae && xcodebuild -project bae.xcodeproj -scheme bae \
    -derivedDataPath .build/derivedData build
  ```

  Disconnect VPN first — incomplete fetches corrupt the SwiftPM cache.

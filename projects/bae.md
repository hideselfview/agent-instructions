# bae

Personal music library + playback. Rust core (bae-core), platform UIs via uniffi
(bae-bridge → bae-macos, bae-ios, bae-android, bae-web), and a cloud sync layer
(bae-proxy) supporting S3 / Google Drive / Dropbox / OneDrive.

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

## SPM cache recovery (bae-macos)

The `post-checkout` hook (installed via `scripts/install-hooks.sh`, source in
`scripts/hooks/post-checkout`) primes the SPM cache on new-worktree creation:
`xcodegen generate` + `xcodebuild build` with the same flags the pre-commit hook
uses (`-scheme bae`, `-derivedDataPath .build/derivedData`). The flags must
match — if xcodebuild resolves packages to the default DerivedData location, the
pre-commit hook's xcodebuild can't find them and fails.

If the Sparkle cache gets corrupted (pre-commit fails with "Couldn't check out
revision" or "file not found" on Sparkle):

```sh
rm -rf bae-macos/bae/.build/derivedData
rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/Sparkle*
cd bae-macos/bae && xcodebuild -project bae.xcodeproj \
  -scheme bae -derivedDataPath .build/derivedData build
```

VPN can cause incomplete git fetches that corrupt the SPM cache; disconnect
before re-running if you're on VPN.

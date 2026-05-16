# bae

Personal music library + playback. Rust core (bae-core), platform UIs via uniffi
(bae-bridge → bae-macos, bae-ios, bae-android, bae-web), and a cloud sync layer
(bae-proxy) supporting S3 / Google Drive / Dropbox / OneDrive.

## Stylization: bae is always lowercase

"bae" is always lowercase in user-visible strings — UI text, labels, docs, error
messages, window titles, button text, alt text, meta descriptions, filenames
(`bae.dmg`, `bae Library`), URLs (`bae://`). Never "Bae" or "BAE".

Exception: code identifiers (variables, functions, types) follow language
conventions.

## Greenfield — break things and move on

Pre-1.0. `rm -rf ~/.bae` is the migration strategy. When the canonical shape of
anything changes — DB schema, bae-bridge types, UiEventBus event payloads,
on-disk file layouts, sync membership chain format, encryption schemes, cloud
storage paths — edit the definition and update every caller in one PR. No
migration shims, no dual-shape compatibility flags, no `#[serde(default)]` to
silently absorb renames, no fallback decoders for old data. If a fixture is
stale, regenerate it.

## No real artist/album/song names in artifacts

Never use real artist, album, or song names in any durable written artifact:
code, tests, UI strings, docs, mockups, PR titles/descriptions, commit messages,
plan docs, issue bodies. Use descriptive placeholders that carry the same
meaning — "2×LP vinyl rip", "the release", "Artist Name", "Album Title", "Track
Title", "rel-123".

The only safe place for a real name is ephemeral chat that won't be indexed or
linked later. Before finalizing any PR title/body, commit message, or plan doc,
scan and replace.

Enforced on every PR by `.github/workflows/ai-bae-rules-review.yml`.

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

## UI and bridge thinness (letter of the law)

The generic "UI iterates and renders" rule (in `rules/reactive-ui.md`) is
enforced strictly in bae because cross-platform UI is the goal: macOS today, iOS
/ Android / web ahead. Anything in the UI is something we'd rewrite per
platform.

**bae-bridge is ONLY type translation.** It converts bae-core types ↔
uniffi/Swift types and nothing else. No DB lookups, no API calls, no formatting,
no filtering, no orchestration, no mutable state, no event filtering. If you
need to add functionality, add it to bae-core; the bridge calls it. Never add
"just a quick helper" to the bridge.

Bridge boundary violations to flag:

- Swift computes a derived value from multiple bridge fields instead of
  receiving a pre-computed field (e.g., `badAudioCount > 0 || badImageCount > 0`
  instead of `isIncomplete`).
- Swift formats raw data for display (ms → duration, bytes → size, dates →
  strings) instead of receiving a pre-formatted label.
- Swift switches on bridge string/enum values to make domain decisions (e.g.,
  `source == "musicbrainz"` to build a URL) instead of receiving the result as a
  field.
- Swift sorts/filters bridge arrays using domain rules instead of receiving
  pre-sorted/pre-filtered data.
- Swift groups flat arrays into structured data (tracks by side) instead of
  receiving pre-grouped data.
- Swift constructs URLs, file paths, or identifiers from bridge field values
  instead of receiving them pre-built.
- String literals in Swift that encode domain knowledge (source names, format
  names, status strings) — those are bae-core concepts.

## Error display path

Every surfaced error must have a display path:

- `AppService.lastError` shows via the global alert.
- Per-view errors show inline.
- Import errors show in the candidate's error state.

If you add a new error path, verify it's displayed somewhere before committing.

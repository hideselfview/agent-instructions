# bae

Personal music library + playback. Rust core (bae-core), platform UIs
via uniffi (bae-bridge → bae-macos, bae-ios, bae-android, bae-web), and
a cloud sync layer (bae-proxy) supporting S3 / Google Drive / Dropbox /
OneDrive.

## No real artist/album/song names in artifacts

Never use real artist, album, or song names in any durable written
artifact: code, tests, UI strings, docs, mockups, PR titles/descriptions,
commit messages, plan docs, issue bodies. Use descriptive placeholders
that carry the same meaning — "2×LP vinyl rip", "the release", "Artist
Name", "Album Title", "Track Title", "rel-123".

The only safe place for a real name is ephemeral chat that won't be
indexed or linked later. Before finalizing any PR title/body, commit
message, or plan doc, scan and replace.

Enforced on every PR by `.github/workflows/bae-rules-review.yml`.

## SPM cache recovery (bae-macos)

`scripts/worktree-add.sh` primes the SPM cache for new worktrees by
running `xcodegen generate` + `xcodebuild build` with the same flags
the pre-commit hook uses (`-scheme bae`, `-derivedDataPath
.build/derivedData`). The flags must match — if xcodebuild resolves
packages to the default DerivedData location, the pre-commit hook's
xcodebuild can't find them and fails.

If the Sparkle cache gets corrupted (pre-commit fails with "Couldn't
check out revision" or "file not found" on Sparkle):

```sh
rm -rf bae-macos/bae/.build/derivedData
rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/Sparkle*
cd bae-macos/bae && xcodebuild -project bae.xcodeproj \
  -scheme bae -derivedDataPath .build/derivedData build
```

VPN can cause incomplete git fetches that corrupt the SPM cache;
disconnect before re-running if you're on VPN.

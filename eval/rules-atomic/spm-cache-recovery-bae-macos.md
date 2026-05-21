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

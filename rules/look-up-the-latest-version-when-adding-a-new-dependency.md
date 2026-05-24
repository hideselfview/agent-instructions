---
digest: Check the registry for the current version when adding a dependency; don't guess from memory.
paths:
  - '**/Cargo.toml'
  - '**/package.json'
  - '**/build.gradle*'
  - '**/*.podspec'
blocking: false
---

**Look up the latest version when adding a new dependency.** Don't guess from
memory or copy from elsewhere in the codebase. Check the registry (crates.io,
npm, etc.) for the current version, then pin to that.

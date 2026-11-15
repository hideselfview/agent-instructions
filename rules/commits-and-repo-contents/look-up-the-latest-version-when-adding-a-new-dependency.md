---
digest: When adding a dependency, check its registry for the current version and pin that; don't take a version from memory or from elsewhere in the codebase.
paths:
  - '**/Cargo.toml'
  - '**/package.json'
  - '**/build.gradle*'
  - '**/Package.swift'
  - '**/*.csproj'
  - '**/*.podspec'
blocking: false
review: false  # reviewer has no network to check registries; authoring-only
---

## Look up the latest version when adding a new dependency

When you add a dependency, check its registry (crates.io, npm, Maven Central,
NuGet, or the package's own repository for Swift packages) for the current
version, and pin that. Don't take a version from memory or copy it from
elsewhere in the codebase.

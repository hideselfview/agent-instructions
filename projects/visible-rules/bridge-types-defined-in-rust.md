---
digest: Bridge* types are defined once in visible-bridge/src/types.rs; the Swift and Kotlin versions are generated and gitignored, so read the Rust, never a generated file. An app type that restates a Bridge* type's fields is a duplicate.
paths:
  - visible-bridge/**/*.rs
  - '**/*.swift'
  - '**/*.kt'
blocking: false
---

## Bridge types are defined in Rust and generated for each language

The `Bridge*` records and enums are defined in `visible-bridge/src/types.rs`
(`uniffi::Record`, `uniffi::Enum`). The build generates their Swift and Kotlin
versions, which are gitignored. To check whether a bridge type exists, what its
fields are, or whether an app type duplicates one, read
`visible-bridge/src/types.rs`, never a generated file.

An app struct or enum that restates a `Bridge*` type's fields is a duplicate:
use the generated type. The `Bridge*` types themselves restate core types
because uniffi requires a type of its own at the boundary, so they are not
duplicates.

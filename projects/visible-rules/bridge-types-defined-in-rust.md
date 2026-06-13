---
digest: Bridge* types are defined once in visible-bridge/src/types.rs; the Swift/Kotlin equivalents are generated and gitignored — read the Rust, never the generated file.
paths:
  - visible-bridge/**/*.rs
  - '**/*.swift'
  - '**/*.kt'
blocking: false
---

## Bridge types are defined in Rust, generated per language

The `Bridge*` records/enums live in `visible-bridge/src/types.rs`
(`uniffi::Record` / `uniffi::Enum`). The Swift and Kotlin equivalents are
generated at build time and gitignored. To check a bridge type's existence or
fields, or whether a UI type duplicates one, read `visible-bridge/src/types.rs`
— never conclude from a generated file.

A UI struct/enum that restates a `Bridge*` type's fields is a duplicate: use the
generated type directly. Mirrors mandated by the FFI boundary (the `Bridge*`
types themselves) are not duplicates of the core types — that is the boundary
uniffi requires.

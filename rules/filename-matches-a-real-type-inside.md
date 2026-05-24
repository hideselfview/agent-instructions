---
digest: Every .swift filename must reference a type inside it — primary type, extension target, or caseless-enum namespace.
paths:
  - '**/*.swift'
blocking: false
---

## Filename matches a real type inside

Every `.swift` file's name must reference something that exists in the file — a
primary type, an extension target, or a caseless-enum namespace. Filenames that
don't correspond to anything inside read as "where's the type?" and tend to
attract unrelated dumping over time.

Acceptable shapes:

1. **Single primary type** — `LightboxOverlay.swift` contains
   `struct LightboxOverlay`. Tightly-coupled secondary types (data types the
   primary consumes, sub-views) can coexist.
2. **Extension target** — `BridgeSortField+CaseIterable.swift` contains an
   extension on `BridgeSortField`. Apple's `<Type>+<Aspect>.swift` convention.
3. **Caseless-enum namespace** — `ImageLoader.swift` contains
   `enum ImageLoader { static func load... }`. Use this when grouping free
   functions or related types under a topic — public API inside the enum,
   private helpers at file scope.

When you find a violation:

- Single primary type with mismatched filename → rename file to match.
- Multiple unrelated types under a topic name → split into per-type files.
- Free functions in a topic-named file → wrap public API in
  `enum Topic { static func ... }`.
- Junk-drawer extensions file → split into per-type extension files.

This matters less in tests. A test file may group several `*Tests` suites for
one area under a topic name (e.g. `LibraryStoreTests.swift`); the rule targets
production junk-drawer dumping, not test organization.

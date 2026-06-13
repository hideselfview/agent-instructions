---
digest: visible-bridge is only type translation; UI iterates/renders; no derived values, formatting, or domain decisions in Swift/Kotlin.
paths:
  - '**/*.swift'
  - '**/*.kt'
  - visible-bridge/**/*.rs
blocking: false
---

## UI and bridge thinness

Cross-platform UI is the goal: iOS and Android from one core. Anything in the UI
is something rewritten per platform, so keep both the bridge and the UI thin.

**visible-bridge is ONLY type translation.** It converts visible-core types ↔
uniffi/Swift/Kotlin types and nothing else. No DB lookups, no formatting, no
filtering, no sorting, no orchestration, no mutable state. If you need
functionality, add it to visible-core; the bridge calls it. Never add "just a
quick helper" to the bridge.

**The UI iterates and renders.** It loops over the records the bridge returns
and draws them. It does not compute domain values from multiple fields, format
raw data for display (bytes → size, dates → strings), switch on bridge
string/enum values to make domain decisions, sort/filter/group bridge arrays by
domain rules, or build paths/identifiers from field values. Each of those is a
visible-core concept — receive the result as a field, don't re-derive it in
Swift or Kotlin.

Violations to flag:

- Swift/Kotlin computes a derived value from multiple bridge fields instead of
  receiving a pre-computed field.
- Swift/Kotlin formats raw data for display instead of receiving a label.
- Swift/Kotlin sorts/filters/groups bridge arrays using domain rules instead of
  receiving them pre-shaped.
- String literals in Swift/Kotlin that encode domain knowledge.

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

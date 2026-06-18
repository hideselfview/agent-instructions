---
digest: bae-bridge is only type translation; the UI iterates/renders and does locale-aware formatting of raw data — no derived domain values or domain decisions in the UI.
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - bae-bridge/**/*.rs
blocking: false
---

## UI and bridge thinness (letter of the law)

The generic "UI iterates and renders" rule
(`rules/ui-iterates-and-renders-nothing-more.md`) is enforced strictly in bae
because cross-platform UI is the goal: macOS, iOS, Android, and Windows from one
core. Anything in the UI is something we'd rewrite per platform.

**bae-bridge is ONLY type translation.** It converts bae-core types ↔
uniffi/Swift/Kotlin types (and, via the Windows FFI, the C# wire shapes) and
nothing else. No DB lookups, no API calls, no formatting, no filtering, no
orchestration, no mutable state, no event filtering. If you need to add
functionality, add it to bae-core; the bridge calls it. Never add "just a quick
helper" to the bridge.

**The locale never crosses the bridge.** bae-core and bae-bridge emit raw data —
numbers, typed enums, and stable message keys — never prose or locale-formatted
text. The one computation the UI is *required* to do is locale-aware rendering
of that data: formatting numbers, byte counts, durations, and dates with the
platform's locale formatters, and resolving stable message keys through native
string catalogs (String Catalogs / `strings.xml` / `.resw`). That rendering is
inherently per-platform and OS-owned, so it lives in the UI by design.

Bridge / UI boundary violations to flag:

- The UI computes a derived value from multiple bridge fields instead of
  receiving a pre-computed field (e.g., `badAudioCount > 0 || badImageCount > 0`
  instead of `isIncomplete`).
- The UI switches on bridge string/enum values to make domain decisions (e.g.,
  `source == "musicbrainz"` to build a URL) instead of receiving the result as a
  field.
- The UI sorts/filters bridge arrays using domain rules instead of receiving
  pre-sorted/pre-filtered data.
- The UI groups flat arrays into structured data (tracks by side) instead of
  receiving pre-grouped data — receive the grouping/position as a structured
  field; only the localized words ("Side", "Disc") are the UI's.
- The UI constructs URLs, file paths, or identifiers from bridge field values
  instead of receiving them pre-built.
- String literals in the UI that encode domain knowledge (source names, format
  names, status strings) — those are bae-core concepts and cross as enums/keys.

What is NOT a violation: the UI formatting raw numbers/dates/byte counts for
display, or resolving a localized string by key. That is the required
locale-rendering described above, not a thinness breach — bae-core has no locale
and cannot do it.

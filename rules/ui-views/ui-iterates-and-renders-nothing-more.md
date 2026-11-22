---
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.tsx'
  - '**/*.js'
  - bae-bridge/**/*.rs
  - visible-bridge/**/*.rs
blocking: false
---

## UI iterates and renders, nothing more

UI code, in any UI framework, iterates over the data it receives and renders it.
Domain logic (sorting, filtering, grouping, computing, deriving) belongs in the
data layer: the core, the store or the server. The data layer delivers data in
the shape the UI shows, already sorted and grouped, with its flags computed, so
nothing has to be written again for each platform.

The bridge between the core and the UI only translates types. It converts the
core's types to the types each platform's generated bindings use, such as uniffi
records and enums, and has no logic of its own: no database reads, API calls,
formatting, filtering, orchestration or mutable state. New behavior goes in the
core, and the bridge calls it.

### What the UI computes

- Type conversions for rendering: `Int` to `String` for a text view, `String` to
  `URL` to load an image.
- Formatting for the reader's locale, with the platform's own APIs:
  - numbers, byte counts, durations and dates, with its formatters;
  - messages, looked up by key in its string catalogs.
- A command's target, computed from the value on screen and sent as an absolute
  value: `setShuffle(!shuffled)`, `setRepeat(mode.next())`
  (`set-state-don-t-toggle`).
  - A `!bool` or `mode.next()` names the next state; real logic, such as
    building a URL, still arrives as a field.

### Violations

- The UI computes a value from several fields instead of receiving it as a
  field: `badAudioCount > 0 || badImageCount > 0` instead of `isIncomplete`.
- The UI switches on a string or enum to make a domain decision, such as
  `source == "musicbrainz"` to build a URL.
- The UI sorts or filters a list by domain rules.
- The UI groups a flat list, such as tracks by side, instead of receiving the
  grouping as a field. Only the localized words ("Side", "Disc") are the UI's.
- The UI builds URLs, file paths or identifiers from field values.
- A string literal in the UI encodes domain knowledge: a source name, a format
  name, a status string. These cross as enums or message keys.

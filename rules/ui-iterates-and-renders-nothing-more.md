---
digest: UI iterates and renders; sorting/filtering/grouping/formatting/deriving belongs in the data layer.
paths:
  - '**/*.swift'
  - '**/*.tsx'
  - '**/*.jsx'
  - bae-web/**/*.rs
blocking: false
---

## UI iterates and renders, nothing more

The UI's job is to iterate and render. Domain logic — sorting, filtering,
grouping, computing, formatting, deriving — belongs in the data layer (core,
store, server). The state layer should deliver data in the exact shape the UI
needs: pre-sorted, pre-grouped, pre-formatted, with pre-computed flags. The UI
maps that structure to visual elements.

Exceptions:

- Type coercion for rendering (e.g., `Int → String` for `Text()`, `String → URL`
  for image loading).
- Localized string building and matching (platform-native localization APIs).

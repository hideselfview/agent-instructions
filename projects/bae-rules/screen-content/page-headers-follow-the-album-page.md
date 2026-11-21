---
digest: "A bae page's header follows the album page's pattern: the picture beside a column as tall as it holding the name, then the owner or artist, then the facts, then the actions left-aligned under the text, with secondary actions in the options menu. On macOS the options button sits beside Play; on Android it sits in the top bar."
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
  - bae-bridge/loc/catalog.toml
  - plans/**
blocking: true
design: true
---

## Page headers follow the album page

A page about one thing (an album, a playlist) lays out its header as the album
page does, as the person decided for the playlist page:

- The picture sits beside a column of text, as tall as that column.
- The column holds the name, then the owner or artist on a line of their own,
  then the facts (a track count and length, a year), then the actions.
- The primary action (Play) sits left-aligned under the text. Every other action
  (Shuffle, Edit, Delete, Rate) is in the options menu.
- macOS puts the options button (⋯) beside Play. Android puts the options button
  (⋮) in the top bar at the right, where Android apps put a page's menu
  (decision #1338).
- The header says nothing the person can tell already: a playlist someone else
  shared doesn't say who sees it, since the person sees it only because it is
  shared.

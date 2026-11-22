---
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

## Text in a block shares one left edge

- Text in a block shares one left edge: labels, values, notes and the controls
  under them. A sheet's labels and fields start on the same edge.
- Markers (a track's number, a play's time, a status glyph, a disclosure
  chevron, a bullet) hang in the margin to the text's left and never push the
  text in. A playlist's track numbers and History's times sit in a column left
  of the covers, right-aligned, so the covers and the titles share one edge, and
  the list's headings start at that edge too.
- A row with a leading icon keeps it in the leading column, `size-lead` wide,
  centred, the text starting `space-related` after it on macOS (`space-edge` on
  Android, as Material's list items). On macOS every such row is built from
  `MacItem`, and so is `MacNotice`'s glyph, so wherever a notice sits among
  rows, their words share one edge: the Storage Manager's inspector, the
  Activity panel. An empty leading column stays, so a row without an icon keeps
  the edge too.
- The shared edge holds within one group (a box on macOS, a section on Android)
  and never runs across groups: in the Activity panel the problems' box keeps
  it, and Transfers starts at its own box's inset.
- A marker hangs only into a margin its own block owns, such as a notice's glyph
  column or a list's number column. Where the text edge is a container's edge,
  there is no margin to hang into: the marker goes inside, and a disclosure
  becomes its own row with the chevron inside it.

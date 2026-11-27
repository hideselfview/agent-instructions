---
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
  - bae-bridge/loc/catalog.toml
blocking: true
design: true
---

## Icons and status glyphs

The board decides which icons a screen shows
(`rules/screen-content/icons-only-where-they-help.md`). This rule covers how
they are drawn.

- Android uses Material Symbols (Rounded, weight 400). Apple platforms use SF
  Symbols; the boards draw stand-ins, since a web page can't load SF Symbols.
- A status glyph is a ring with its mark inside (`StatusGlyph`): info `i`,
  success a check, warning and danger `!`, told apart by colour and by the words
  beside them. It has no filled form, so it stays quieter than the words
  wherever it shows. No octagons, no triangles: nothing that reads as a road
  sign.

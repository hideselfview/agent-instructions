---
digest: bae's icons are Material Symbols Rounded 400 on Android and SF Symbols on Apple platforms (stand-ins on boards), shown only as the control or for meaning the words don't carry; a status glyph is a ring with its mark inside, never filled, never a road-sign shape.
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
  - bae-bridge/loc/catalog.toml
  - plans/**
blocking: true
---

## Icons and status glyphs

Icons appear only when they are the control (copy, a chevron, a disclosure mark)
or carry meaning the words beside them don't (a status glyph), as
`rules/screen-content/no-decorative-icons-or-emoji.md` says.

- Android uses Material Symbols (Rounded, weight 400). Apple platforms use SF
  Symbols; the boards draw stand-ins, since a web page can't load SF Symbols.
- A status glyph is a ring with its mark inside (`StatusGlyph`): info `i`,
  success a check, warning and danger `!`, told apart by colour and by the words
  beside them. It has no filled form, so it stays quieter than the words
  wherever it shows. No octagons, no triangles: nothing that reads as a road
  sign.

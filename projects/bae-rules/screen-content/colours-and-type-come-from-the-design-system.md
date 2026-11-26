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

## Colours and type come from the design system

The apps offer six tones (neutral, slate, plum, midnight, forest, sand), six
accents, and light and dark modes. The design system holds the midnight tone and
the pink accent, the ones a new installation starts with, in Dark and Light;
`design/system/tokens.css` gives each colour's use beside it.

- Every colour on a platform comes from the design system: the accent the person
  chose, the surfaces, the text. None is left to the system's own colour (the
  Mac's accent, Material's dynamic colour) beside them; a part the platform
  colours with no way to change it is drawn by the app, and when the app can't
  draw it, ask the person.
- The design system measures the accents in OKLCH
  (`the-accents-are-measured-in-oklch.md`, for `design/**`), and
  `design/system/tokens.json` holds each value.
- How the colour names read: a bare name (`accent`, `accent-blue`, `danger`,
  `info`) is text and glyphs on background, surface and elevated. `-fill` is a
  filled shape, and `on-…-fill` is what sits on it. `-selection` is a selected
  row, and `-tint` a faint wash behind something chosen. `accent`,
  `accent-fill`, `on-accent-fill` and `accent-selection` are the chosen accent;
  the boards show pink.
- Each status carries a word, never colour alone.
- Secondary text draws in the design system's `text-secondary` on every
  platform, macOS included. The Mac app and the Mac boards never use the
  system's own secondary label colour (NSColor.secondaryLabelColor).
- The text styles are macOS points. iOS follows Dynamic Type, and the Android
  components set Android's sizes. No font files ship: each platform uses its own
  face. `title`, `hero` and `display` are for pages and the welcome screen, not
  settings.
- A headline wraps balanced, so it never leaves one word alone on its last line,
  in any language, and a line under it wraps the same way.

---
digest: Every colour bae shows comes from the design system's tokens (the chosen tone and accent, the surfaces, the text), never from the platform; accents are measured in OKLCH; each status carries a word, never colour alone; Mac secondary text is the system's own colour; text styles are the design system's, with title, hero and display only for pages and Welcome.
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

## Colours and type come from the design system

The apps offer six tones (neutral, slate, plum, midnight, forest, sand), eight
accents, and light and dark modes. The design system holds the midnight tone and
the pink accent, the ones a new installation starts with, in Dark and Light;
`design/system/tokens.css` gives each colour's use beside it.

- Every colour on a platform comes from the design system: the accent the person
  chose, the surfaces, the text. None is left to the system's own colour (the
  Mac's accent, Material's dynamic colour) beside them; a part the platform
  colours with no way to change it is drawn by the app.
- The accents are measured in OKLCH, a colour space where equal steps in the
  numbers look equal. Each fill has the sheep's pink lightness, 0.75, and its
  saturation, 0.17, or the most the screen can show at that hue. The hues start
  at the sheep's pink (353°), and the other seven sit where each is the same
  visible distance from its neighbours: blue 268°, orchid 313°, pink 353°, coral
  36°, gold 81°, lime 123°, mint 166°, sky 213°.
- Text on an accent fill is the same hue at lightness 0.28 and saturation 0.09.
- An accent as text and glyphs (`accent-blue` and the like) is the hue at
  lightness 0.78 and saturation 0.11 in Dark, and at lightness 0.53 and
  saturation 0.16 in Light, made darker where needed to reach 4.5:1 on
  background, surface and elevated. A selected row is the Light text colour in
  both modes, with white text, quieter than the button.
- How the colour names read: a bare name (`accent`, `accent-blue`, `danger`,
  `info`) is text and glyphs on background, surface and elevated. `-fill` is a
  filled shape, and `on-…-fill` is what sits on it. `-selection` is a selected
  row, and `-tint` a faint wash behind something chosen. `accent`,
  `accent-fill`, `on-accent-fill` and `accent-selection` are the chosen accent;
  the boards show pink.
- Each status carries a word, never colour alone.
- Secondary text on macOS is the system's own colour (`mac-text-secondary`,
  measured from NSColor.secondaryLabelColor), which follows Increase Contrast
  and translucent backgrounds there; the Mac code uses it directly.
  `text-secondary` is Android's. Every Mac window draws the system colour, and a
  Mac board or preview that draws a component outside a window is marked
  `bae-mac`.
- The text styles are macOS points. iOS follows Dynamic Type, and the Android
  components set Android's sizes. No font files ship: each platform uses its own
  face. `title`, `hero` and `display` are for pages and the welcome screen, not
  settings.
- A headline wraps balanced, so it never leaves one word alone on its last line,
  in any language, and a line under it wraps the same way.

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
  colours with no way to change it is drawn by the app.
- The accents are measured in OKLCH, a colour space where equal steps in the
  numbers look equal. Each fill has the sheep's pink lightness, 0.75, and its
  saturation, 0.17, or the most the screen can show at that hue. The six hues
  are blue 268°, orchid 313°, pink 353° (the sheep's pink), lime 123°, mint 166°
  and sky 213°. Going round the hue circle, the gaps are 45° from blue to
  orchid, 40° to pink, 130° to lime, 43° to mint, 47° to sky and 55° back to
  blue. The spacing is not equal: the gap from pink to lime is wider than the
  others, because the picker has no coral or gold and leaves the red-orange and
  amber hues to the danger and warning colours.
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
- Secondary text draws in the design system's `text-secondary` on every
  platform, macOS included. The Mac app and the Mac boards never use the
  system's own secondary label colour (NSColor.secondaryLabelColor).
- The text styles are macOS points. iOS follows Dynamic Type, and the Android
  components set Android's sizes. No font files ship: each platform uses its own
  face. `title`, `hero` and `display` are for pages and the welcome screen, not
  settings.
- A headline wraps balanced, so it never leaves one word alone on its last line,
  in any language, and a line under it wraps the same way.

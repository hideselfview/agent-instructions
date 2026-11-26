---
paths:
  - design/**
blocking: true
design: true
---

## The accents are measured in OKLCH

The design system's six accents follow these measures, and
`design/system/tokens.json` holds the values they give. Every colour on a
platform comes from those tokens
(`colours-and-type-come-from-the-design-system.md`).

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

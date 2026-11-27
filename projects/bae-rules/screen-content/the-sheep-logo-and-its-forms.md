---
paths:
  - design/**
  - assets/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
blocking: true
design: true
---

## The sheep logo and its forms

The sheep is bae's logo: `assets/Logos/bae-sheep.svg`, with no background of its
own. It's the product's own artwork, so the icon rules don't apply to it.

- **Sheep and "bae" together:** where bae introduces itself: Welcome, About and
  the design system's cover. The sheep on top, both centred, the word
  `assets/Logos/bae-word.svg` at 0.52 of the sheep's width. The word is Fredoka
  SemiBold turned into a drawing, so it's the same on every platform and no app
  needs the font. Fredoka is for the logo only.
- **Sheep and "bae" in a line:** where there's width but little height, such as
  a header. The sheep comes first: on the left, or on the right in a
  right-to-left language.
- **The tagline:** "Your music, everywhere". It sits under the logo on Welcome,
  in secondary text with no period, and stays still while the logo floats.
- **The bob:** on Welcome the sheep and the word float separately, the rise and
  the tilt on timers of their own so the motion doesn't visibly repeat. Each
  rises by 4% of the sheep's height every 6 seconds and tilts 2° each way every
  7.3, the word 2 seconds behind the sheep. It stops when the person has asked
  for reduced motion.
- **The glow:** on Welcome a pink and purple disc of light sits still behind the
  sheep and the word, 2.6 times the sheep's width, in the website's colours,
  fading out with no visible edge, with a pool of the same light under the
  sheep. The word is `logo-word`, the wool's cream in dark mode and the
  outline's plum in light mode, with a soft white 1px edge to the lower right,
  at 45°.
- **Sheep alone:** where the system or the space already says the name: app
  icons and favicons. On an app icon it sits on a pale pink tile, `#FFD6E7`.
- **The sheep at play:** on the empty Import screen the sheep, alone, plays a
  short moment every few seconds, most of them taking in made-up releases
  (`ImportSheep`). It is the logo's own drawing moved in parts, never mirrored,
  and holds still when the person has asked for reduced motion.
- **Small sheep:** at 32px and under, the full drawing loses its detail, so
  favicons and small icons use `bae-favicon-16.svg` and `bae-favicon-32.svg`,
  drawn pixel by pixel for those sizes. Show them at exactly those sizes.
- **"bae" alone:** the drawn word, `assets/Logos/bae-word.svg`, where a logo
  belongs but the sheep doesn't fit.
- **"bae" in text:** in sentences, titles and menus the name is plain text in
  whatever font is around it, never the drawn word.

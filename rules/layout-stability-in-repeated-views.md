---
digest: In ForEach/List rows keep all content in the tree; toggle with opacity + hit-testing, not if-inclusion.
paths:
  - '**/*.swift'
blocking: false
---

## Layout stability in repeated views

Inside `ForEach`/`List` rows (or any repeated container), keep all possible
content in the layout tree at all times. Use `.opacity(0/1)` plus
`.allowsHitTesting(false/true)` to show/hide. Never
`if condition { SomeView() }` to conditionally include a child — that changes
the row's intrinsic size, dirties the parent stack, and forces recursive
re-measurement of every sibling row.

Symptom: a list of 100+ items with hover-revealing buttons drops to ~0.5 FPS on
hover. The hover flips `@State isHovered`, which conditionally adds a button to
a ZStack, which dirties the ZStack's intrinsic size, which dirties the VStack,
which re-lays-out all rows.

Outside repeated views (a single sheet, a one-off detail view), conditional
inclusion is fine.

---
digest: A design-system component's preview is a set of named rows, each changing one state or option of the same example with the same words, one row per value, no combinations, and a container shows a dashed stand-in for its content.
paths:
  - design/system/**
blocking: true
design: true
---

## Component previews hold their content still

A component's preview is a set of named rows, each changing one thing.

- Hold the content still. Every row shows the same example with the same words,
  so the only difference is the state or option the row is named for. No
  exceptions for a field or a word that seems a better fit.
- One row per state or option, named the way you'd ask for it in code: Resting,
  Focused, Error, Off.
- When an axis is done, look for the next among the component's other props.
  Show each value once; don't multiply axes into combinations.
- A container holds a plain stand-in, a dashed box, so the container is the only
  real thing on show.

---
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.tsx'
  - '**/*.ts'
  - '**/*.html'
  - '**/*.css'
  - '**/*.axaml'
  - '**/*.cs'
  - design/**
blocking: true
design: true
---

## No slop chips

Chips are allowed. LLM slop chips are banned: a colored pill on every row or
card that labels its status, kind or category ("Working", "Ready to land",
"Held", "Waits on you", "New", "Beta", a category tag), until a column of them
runs down the edge. The color then does the layout's job, and the one row that
needs attention is lost among the rows that don't.

A chip is a slop chip when any of these hold:

- **Every row has one.** A chip on each item is a column of labels, not a
  signal.
- **It repeats.** The heading, the row's words or its position already say what
  the chip says.
- **It replaces structure.** The rows have states that should group them, and
  the chips carry the grouping instead.
- **It decorates.** Its color or shape makes the row look finished and tells the
  reader nothing.

Show state with structure and words instead:

- **Group by state.** Put rows under a heading per state, the one that needs
  someone first ("Waits on you", then "Working"). The heading says the state
  once, and the rows under it say nothing more about it.
- **Say it in the line.** When a row's state is the news, the row's own words
  say it, as part of the sentence.

A chip is right when it is:

- **A control:** a filter you toggle, a token you remove from a field, a choice
  in a row of options.
- **The exception:** one or a few items that differ from the rest in a way the
  reader must notice and nothing else on the screen says.
- **A count** on a tab or a heading ("Work 93").

When you remove slop chips, remove what existed to hold them (the right-hand
column, the color tokens for each row), and check whether the rows now need
grouping.

---
digest: Chips are allowed; LLM slop chips are banned - a colored pill on every row or card restating its status, kind or category. Group rows under a heading per state or say the state in the line's words; a chip marks only the exception nothing else on screen says.
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
---

## No slop chips

Chips aren't banned. LLM slop chips are: the colored pill that goes on every row
or card to label its status, kind or category ("Working", "Ready to land",
"Held", "Waits on you", "New", "Beta", a category tag), until a column of them
runs down the edge. Every row shouts, the color says what the layout should, and
the one row that needs attention hides among the many that don't.

It is a slop chip when any of these hold:

- **Every row has one.** A chip that appears on each item is a column of labels,
  not a signal.
- **It restates.** The heading, the row's words or its position already say what
  the chip says.
- **It stands in for structure.** The rows have states that should group them,
  and the chips carry the grouping instead.
- **It decorates.** Its color or shape is there to make the row look finished,
  not to tell the reader anything.

Show state with structure and words instead:

- **Group by state.** Rows sit under a heading per state, the one that needs
  someone first ("Waits on you", then "Working"). The heading says the state
  once; the rows under it say nothing more about it.
- **Say it in the line.** When a row's state is the news, its own words say it,
  as part of the sentence.

A chip earns its place when it is:

- **A control:** a filter you toggle, a token you remove from a field, a choice
  in a row of options.
- **The exception:** the one or few items that differ from the rest in a way the
  reader must notice and nothing else on the screen says.
- **A count** on a tab or a heading ("Work 93").

When removing slop chips, remove what existed to hold them (the right-hand
column, the per-row color tokens), and check whether the rows now need grouping.

Example (builds dashboard board, 2026-11-13): each in-flight row ended with a
pill, "Waits on you", "Working", "Ready to land", "Held: screen locked", down
the right edge. The person: "we are going to do an agent-instructions level very
prominent ban on what i call LLM slop chips salad … it's banned", and "chips
aren't banned, but LLM slop chips are definitely banned".

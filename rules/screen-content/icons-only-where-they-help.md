---
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.tsx'
  - '**/*.js'
  - '**/*.ts'
  - '**/*.html'
  - '**/*.css'
  - '**/*.axaml'
  - '**/*.cs'
  - '**/*.astro'
review: false
---

## Icons only where they help

The board decides which icons a screen shows, and the person decides the board.
An agent building a screen draws the icons its board draws and adds none of its
own. When no board covers what you are building, add an icon only where it helps
a person recognize or understand something faster than the words alone.

Icons that help:

- **The control itself:** a play button, a close button, a disclosure chevron.
- **A status the words don't say:** a status glyph (synced, failed,
  downloading), a warning mark on the row that failed.
- **A thing people know by its mark:** a platform's logo on a download page, a
  globe on a language picker, the devices in a diagram of how sync works.

Icons that are filler:

- A stock symbol above the title of a welcome, onboarding or sign-in screen.
- A symbol at the top of an empty state: "No albums yet" needs no folder drawing
  above it, since the sentence and its action are the empty state.
- A symbol beside a label in a list of text rows, such as a gear beside
  "Settings" or a trash can beside "Delete" in a menu of words.
- A generic glyph in place of a missing image: a missing cover or photo is a
  plain tile, not a stock picture of a note, a person or a photo frame.

The product's own artwork (its logo, mascot or wordmark) is never filler.

When you remove a filler icon, remove what existed only to hold it (its frame,
its padding, a stack left with one child), rather than leaving the space it
took.

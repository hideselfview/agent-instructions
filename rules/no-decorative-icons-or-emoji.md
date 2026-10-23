---
digest: No decorative icons or emoji; an icon appears only where it is the control or says something the text beside it doesn't.
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.tsx'
  - '**/*.ts'
  - '**/*.html'
  - '**/*.css'
  - '**/*.axaml'
  - '**/*.cs'
  - '**/*.md'
blocking: true
---

## No decorative icons or emoji

An icon earns its place only by doing a job the words can't:

- **It is the control:** a play button, a close button, a disclosure chevron.
- **It carries meaning the text beside it doesn't:** a status glyph (synced,
  failed, downloading), a warning mark on the row that failed.

Everything else is decoration, and decoration is banned:

- A stock symbol (an SF Symbol, a Material icon) above the title of a welcome,
  onboarding or sign-in screen.
- A symbol at the top of an empty state ("No albums yet" needs no folder drawing
  above it; the sentence and its action are the empty state).
- A symbol repeating what its label already says (a gear beside "Settings" in a
  list of text rows, a trash can beside "Delete" in a menu of words).
- A generic glyph filling a missing image: a missing cover or photo is a plain
  tile, not a stock picture of a note, a person or a photo frame.

The product's own artwork (its logo, mascot or wordmark) is not decoration; the
ban never removes it.

Emoji are banned everywhere the user or a reader sees text: UI strings,
notifications, error messages, docs, commit messages, PR descriptions and logs.

When removing a decorative icon, remove what existed only to hold it (its frame,
padding, a stack with one child left) rather than leaving the space it took.

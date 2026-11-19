---
digest: "bae's screens use few words, in sentence case: a full sentence ends in a period and a fragment has none, buttons are verbs, a cut line ends or breaks in \"…\" by what it holds, placeholders never repeat the label, names take no quotes, and no screen has exclamation marks, emoji, marketing words or a storage company the person did not choose."
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
  - bae-bridge/loc/catalog.toml
  - plans/**
blocking: true
---

## Screen words say only what the UI cannot

- Write sentence case everywhere on screen: "Set up sync", "Restore code".
- Use few words, phrases and sentences, and only to say what the UI itself
  can't. A note is one sentence; most controls need none.
- A full sentence ends in a period: "People in this library see this name." A
  label or fragment has none: "No shared folders yet".
- Cut one-line text that doesn't fit with "…": a file or folder name in the
  middle, so its start and its end both show (`Live at the Ha…1994-03-12.flac`);
  a title, a name and a sentence at the end, since they read from the start
  ("The import could not be…"). The whole text shows where the row opens.
- A button is a verb: "Set up", "Cancel", "New folder…". An ellipsis on a button
  means it opens more before acting; on a waiting line it means work is under
  way: "Checking the folder…".
- A placeholder never repeats the label and never ends in an ellipsis. With a
  visible label, leave the empty field blank, unless the placeholder says what
  the field is for that the label can't: "Appears on your profile, ratings and
  playlists" under the label "Your name". It fits the box on one line, about 40
  characters on a phone. Example values are plain and generic: `my-bucket`,
  `https://s3.home.lan`.
- A question about a field goes in the field's helper line, not in a sentence
  above it.
- Put no quotes around names or the names of controls: where a thing sits, or
  the control itself, says which.
- Name no storage company where the choice isn't the person's to make. Use no
  exclamation marks, no emoji and no marketing words.

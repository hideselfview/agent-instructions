---
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
  - bae-bridge/loc/catalog.toml
  - plans/**
blocking: true
design: true
---

## Dialogs, sheets and popovers by what they ask

Dialogs, sheets and alerts go by what they ask of the person:

- **A confirmation** (confirm, acknowledge an error): macOS `MacAlert`; Android
  `AndroidDialog`; iOS an alert, or a confirmation dialog for a destructive
  choice.
- **A pick from a short list** (a rating, a sort order): macOS a menu or a
  submenu of the menu that holds the action; Android `AndroidSheet` listing the
  choices, its current one checked, which closes when the person picks one, as
  the person decided.
- **Something to look at, nothing to decide** (a library's details, the queue,
  the cast picker, the Activity list): Android `AndroidSheet`, closed by swiping
  down or tapping outside, with no Close button; iOS a sheet at its medium
  height; macOS a popover hanging off what it describes.
- **A task with fields or steps** (invite, add a device, set up sync): macOS a
  sheet on the window (`MacModal`); Android a dialog when it is one or two
  fields, its own screen when it is more; iOS a sheet with Cancel and Done,
  since iOS puts forms in sheets.

Three ways fold things away, each for its own job:

- **A disclosure row** folds part of one group: Advanced settings, a short list
  most people don't need. Every platform has one, in its own density. macOS:
  `MacDisclosure`, its own box, the chevron leading inside the row, pointing
  right and turning down when open, the rows it shows indented to the label.
  Android: `AndroidDisclosure`, a row in its own card, `expand_more` at the end
  when closed and `expand_less` when open, the rows it shows joining the card on
  the same edge.
- **A collapsible section** folds a whole section of a long screen: its heading
  opens and closes it. Android only, the section label with `expand_more` and
  `expand_less` at its end. macOS has no collapsible section for content; there
  it is a disclosure row.
- **A sidebar heading** folds a group of destinations in a macOS sidebar, its
  chevron showing only under the pointer. It is navigation, not content.

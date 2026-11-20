---
digest: bae's screens use the design system's components for their platform (Mac… on macOS, Android… on Android; Windows and Linux follow macOS); Mac settings are sections over boxes of rows in the settings window, Android screens are sections over cards of rows with changes in a dialog; a wait with no length shows the platform's own spinner; an empty list is one sentence in its box.
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

## Each platform draws with its own components

- Components come per platform: `Mac…` for macOS, `Android…` for Android;
  `StatusChip` serves both. Windows and Linux follow macOS; iOS has no
  components of its own. Each component's README ends with its counterpart in
  the code; "none yet" means the code needs one.
- On macOS, settings are `MacSection` headings over `MacBox`es of `MacRow`s
  inside a `MacSettingsWindow`. A row that opens something ends in a chevron. A
  Mac list adds to itself with a button under its box (`MacSheetFooter`: "Add
  library…", "Invite…"), as System Settings puts "Add Account…", never with a
  row inside the box.
- On Android, a screen is an `AndroidScreen` of `AndroidSection` labels over
  `AndroidCard`s of `AndroidRow`s; changing a value opens an `AndroidDialog`.
- A wait with no length shows each platform's own spinner: macOS's spokes
  (`MacSpinner`), Material's ring (`AndroidSpinner`). A spinner is a signal
  people already read as busy and carries nothing of bae's, so it is not made
  the same across platforms.
- An empty list is one sentence in its box: "No shared folders yet". No glyph
  sits above it.

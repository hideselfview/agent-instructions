---
paths:
  - bae-macos/**
  - bae-android/**
  - bae-ios/**
blocking: true
---

## No text recognition in tests

A test never captures a view and reads its words back with text recognition
(Vision's `VNRecognizeTextRequest`, ML Kit, or any OCR). The recognizer changes
with each OS version and can read the same pixels differently ("Waiting…" as
"Waitina…"), so the test fails on a view that is right.

To check what a view says, read its words where they are kept: its accessibility
labels and values, or the values it renders from. To check how it looks, add or
change a shot: a macOS scene in `bae-macos/bae/baeShots/`, or an Android shot in
`bae-android/app/src/test/java/fm/bae/app/shots/`. A shot draws the real view
and compares it with its board.

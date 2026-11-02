---
digest: Tests never read text off a screenshot with text recognition (Vision OCR); they check words through accessibility or the view's values, and how a screen looks is baeShots' job.
paths:
  - bae-macos/**
  - bae-android/**
  - bae-ios/**
blocking: true
---

## No text recognition in tests

A test never captures a view and reads its words back with text recognition
(Vision's `VNRecognizeTextRequest`, ML Kit, or any OCR). The recognizer changes
with the OS: CI's macOS 26.6.2 read a chip's "Waiting…" as "Waitina…" from the
same pixels macOS 26.3.1 read right, so the test failed on a view that was
right.

To check what a view says, read its words where they are kept: its accessibility
labels and values, or the values it renders from. To check how it looks, add or
change a baeShots scene (`bae-macos/bae/baeShots/ShotScene.swift`,
`bae-android/app/src/screenshotTest/.../Shots.kt`), which renders the real view
for review.

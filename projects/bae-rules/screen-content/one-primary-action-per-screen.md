---
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
  - bae-bridge/loc/catalog.toml
blocking: true
design: true
---

## One primary action per screen

- Each screen has one primary button: the action the screen is for, on the right
  of the footer. Everything else is secondary.
- One button in a row has colour, never two: the primary's fill (the accent on
  Android), or, when the action deletes or wipes something that can't come back
  (Delete an album, Reset this device), that one in red. Red is for loss.
  - Ending an invite, declining a request or cancelling a wait is not loss, and
    stays in the accent or plain.
  - Removing a library from this device isn't loss either (it stays in the cloud
    and on other devices): its Remove is the primary button, in the accent.
  - Every other button is plain: Cancel, Back, Not yet, Cancel invite beside
    Done, Decline beside Approve. Two accent buttons, or accent beside red, is
    never right.
- A sheet's Cancel goes back one screen, and closes the sheet on the first one.
  No Back buttons.
- Don't hide a button that can come back; turn it off.
- A Remove at the end of a row is borderless and destructive.

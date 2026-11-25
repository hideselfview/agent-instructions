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

## Notices say what happened and the fix

A notice says what happened, what's wrong and the fix, each once:

- Where it sits names its subject.
- The title is what happened, in a word or two.
- The line is what's wrong, in words any person understands, with no mechanism
  and no claim the code doesn't back.
- A button is the fix, named for what it does, never a pointer to a control
  elsewhere. The confirm it opens says what the fix costs.
- The technical cause is behind Details.

Read every state as the person: does it worry them? If it does, its line says
what to do about it, in words they can act on (open bae on another device, free
space, sign in), and its button does it where bae can. Don't draw a worrying
state that the person cannot act on: stop and ask the person to decide. A state
that clears by itself says nothing alarming and asks nothing. Each state's line
tells what happened to their music or their changes, not what the software did.

- A notice never shows the error itself. When there is an error behind it, its
  Details button opens an alert (`MacAlert` with `details`) or a dialog
  (`AndroidDialog` holding `AndroidErrorDetails`) with the explanation and the
  causes, one a line, with Copy. In the Activity panel, clicking (tapping) the
  problem opens it instead.
- Inside an alert or dialog a notice is always `quiet` (no fill, the glyph in
  its tone): a filled box in a dialog stands out from everything else in it.
  "Unsent changes will be lost" in the removal confirmation is one.
- An error in part of a screen is its box; a Retry that belongs to it is the
  box's action, at its right, level with the message.
- A view that failed whole is a notice in the centred layout, quiet: a small
  grey outline glyph before the message, then Details and Retry on one row under
  it, Retry last.

### An example

A library on the Mac's Welcome list fails to open because its tables don't match
the app's.

- Before, a notice over the whole list: "Library failed to open / Choose another
  library, or restore it with “Enter code”." It doesn't say which library, and
  Enter code isn't on the screen.
- Rejected: "Couldn’t open “Home music”" (the row already names it); "This Mac’s
  copy can’t be read" (copy of what?); "It doesn’t match what this version of
  bae reads … the library in the cloud is untouched." (the mechanism); "The
  library is safe in the cloud." (a claim nobody checked, in an advert's voice);
  "Load it again" (a name the app doesn't use); "Open another library" (true,
  and no help); "Edits that haven’t synced are lost." (the confirm says it).
- Final, joined under the library's row: the title Could not open, the line
  "Something is wrong with this library’s data.", and the buttons Details and
  Reset from cloud…, whose confirm warns about edits that haven't synced.

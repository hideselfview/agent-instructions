---
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.xml'
  - '**/*.xcstrings'
  - '**/*.dc.html'
  - design/**
blocking: false
design: true
---

## UI text says each thing once, in gentle, plain words

**Once per screen.** Read the screen's text top to bottom as one piece. A phrase
that appears twice ("the person who invited you" in the headline and again under
the code) is once too many: keep it where it does the work, and use a pronoun or
nothing in the other place ("Waiting for them to let you in…").

**The gentler verb for what people do to each other.** When the text describes
another person acting on the reader, pick the word a friend would use, not the
system's: "let you in", not "approve you"; "invite them again", not "resend the
authorization". The system's word can stay in code and logs.

**No word alone on a headline's last line.** Wrap headlines balanced and body
text without orphans, in every language, with the platform's line breaking,
never with breaks placed by hand:

- CSS: `text-wrap: balance` for headlines, `text-wrap: pretty` for body text.
- Compose: `LineBreak.Heading` and `LineBreak.Paragraph`.
- AppKit: `NSParagraphStyle.lineBreakStrategy` `.pushOut`.

**An action that hasn't happened yet takes "will".** A confirmation describes
what happens after the person confirms, so its line says what the app will do.
Don't put that coming action in the simple present: a reader takes "The app
deletes 540 MB" to mean the deletion is already happening. The slip usually
comes from turning a passive sentence ("540 MB will be removed") into an active
one and dropping the "will" with the passive.

- ✗ "The app deletes 540 MB from this phone." → ✓ "The app will delete 540 MB
  from this phone."
- ✗ "The app deletes your saved key." → ✓ "The app will delete your saved key."

The simple present stays right for what the app always does ("The app keeps the
files you opened here") and for what a line states as true now ("Your library
stays as it is").

**A placeholder says what a field is for, only when the label can't.** Under the
label "Email", "Enter your email" repeats the label. A placeholder fits the box
on one line, about 40 characters on a phone.

**A notice says what happened, what's wrong and the fix, each once.**

- Where the notice is shown names its subject: a notice under a library's row
  doesn't repeat the library's name.
- The title says what happened, in a word or two ("Could not open").
- The line says what's wrong, in words any user understands, with no mechanism
  and no claim the code doesn't back.
- The button is the fix, named for what it does.
  - Never send the reader to a control somewhere else, least of all one not on
    the screen.
- The cost of the fix ("edits that haven’t synced are lost") is said where the
  person commits to it: in the confirmation the button opens, not again here.
- The technical cause is behind Details.
- No quotes around names or the names of controls.
- No contractions: write the words out (Could not, does not, cannot).

Example: a list's row for a folder that the app can no longer read.

- Before, a notice over the whole list: "Folder failed to load / Choose another
  folder in Settings." It doesn't say which folder, and Settings isn't on the
  screen.
- After, under the row: "Could not open / The app cannot read this folder." with
  Details and Choose again…

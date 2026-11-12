---
digest: Screen text says each thing once (no phrase repeated across a screen), uses the gentler verb for what people do to each other, and headlines wrap balanced so no word hangs alone.
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.xml'
  - '**/*.xcstrings'
  - '**/*.strings'
  - '**/*.dc.html'
  - design/**
blocking: false
---

## UI text says each thing once, in gentle, plain words

**Once per screen.** Read the screen's text top to bottom as one piece. A phrase
that appears twice ("the person who invited you" in the headline and again under
the code) is one too many: keep it where it does the work and use a pronoun or
nothing in the other place ("Waiting for them to let you in…").

**The gentler verb for what people do to each other.** When the text describes
another person acting on the reader, pick the word a friend would use, not the
system's: "let you in", not "approve you"; "invite them again", not "resend the
authorization". The system's word can stay in code and logs.

**Headlines don't leave a word alone on their last line.** Wrap headlines
balanced and body text without orphans, in every language, by the platform's
line breaking, never by hand-placed breaks: CSS `text-wrap: balance` (headlines)
and `text-wrap: pretty` (body); Compose `LineBreak.Heading` and
`LineBreak.Paragraph`; AppKit `NSParagraphStyle.lineBreakStrategy` `.pushOut`.

**A placeholder says what a field is for only when the label can't.** Under the
label "Your name", "For your profile, ratings and playlists" earns its place;
"Enter your name" repeats the label. It fits the box on one line, about 40
characters on a phone.

**A notice says what happened, what's wrong and the fix, each once.**

- Where it sits names its subject: a notice under a library's row doesn't repeat
  the library's name.
- The title is what happened, in a word or two ("Couldn’t open").
- The line is what's wrong, in words any user understands. No mechanism, and no
  claim the code doesn't back.
- The button is the fix, named for what it does. Never point the reader at a
  control somewhere else, least of all one not on the screen.
- The cost of the fix ("edits that haven’t synced are lost") is said where the
  person commits: in the confirm the button opens, not again here.
- The technical cause is behind Details.
- No quotes around names or the names of controls.

Worked example, a library on the Mac's Welcome list that fails to open because
its tables don't match the app's:

- Before (a notice over the whole list): "Library failed to open / Choose
  another library, or restore it with “Enter code”." It doesn't say which
  library, and Enter code isn't on the screen.
- Rejected: "Couldn’t open “Home music”" (the row already names it); "This Mac’s
  copy can’t be read" (copy of what?); "It doesn’t match what this version of
  bae reads, so it can’t open as it is. Loading it again from the cloud makes a
  new copy here; the library in the cloud is untouched." (the mechanism); "The
  library is safe in the cloud." (a claim nobody checked, in an advert's voice);
  "Load it again" (a name the app doesn't use); "Open another library" (true,
  and no help); "Edits that haven’t synced are lost." (the confirm says it).
- Final, joined under the row: "Couldn’t open / Something is wrong with this
  library’s data." with Details and Reset from cloud…, whose confirm warns about
  unsynced edits.

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

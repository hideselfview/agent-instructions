# Drawing screens

How a screen in a flow is laid out: a step of joining, signing in, adding a
device, an outcome to acknowledge. These hold on every platform; each platform
draws them with its own parts.

## One thing per screen, said once

- The headline says what to do or what happened: "Tell the person who invited
  you this code", "This invite has ended". A screen that needs a sentence and a
  second sentence to explain itself is two things; cut one, or move it where it
  applies (a question about a field goes in the field's helper line or
  placeholder).
- The bar or window title says where you are ("Join library"); the headline
  doesn't repeat it, and the title doesn't repeat what the headline says.

## The action follows what it acts on

- A button sits right under the field, code or line it acts on, a section apart.
  It is never pinned to the bottom of the screen across an empty gap: on a tall
  screen the person looks for it, and on a phone with the keyboard up the gap is
  all that's left.
- A screen to read or fill in (a form, an outcome) starts at the top and reads
  down, left-aligned. Centre a screen only when its content is one thing to look
  at, such as a code to read aloud.

## A wait shows it's alive and finishes on its own

- While a screen waits on something outside it (another device, another person),
  it checks by itself while it's on screen, every few seconds, and shows a
  spinner beside a line saying what it waits for ("Waiting for your other
  device…"). No countdown to the next check, no "Check now" button: a person
  shouldn't have to drive the app's polling.
- Don't show a deadline the person can do nothing about ("waiting up to 7 more
  days"). When the wait ends, the screen that follows says so and what to do.

## Parts, not drawings

- Anything that appears on more than one screen (a code, a step's layout, a row
  of buttons) is a design-system part, used everywhere it appears, so the
  screens can't drift apart. Drawing it by hand on a board is the bug, even when
  the pixels match.
- Before adding a part, look for the one the system already has (a sheet's
  button row, a spinner with a label) and use or extend it.

## The person reviews in the sidebar

- While drawing, `builds design render DIR BOARD [--theme light|dark]` prints a
  board of the worktree as one page of HTML, drawn with the worktree's design
  system; open it to see the board without publishing the canvas.
- When the boards are drawn, the design session records the design change
  (`builds design record`: what changed, each board's canvas link, and how the
  review page reads: its headings, words and the boards under each, with the
  theme of each). It shows in the builds dashboard sidebar's Waiting on you tab
  as a light card, waiting on the person's review; its Details opens the review
  page in a modal. Every board on that page is drawn live from the design
  session's worktree, in a frame the person can zoom, and drawn again on each
  load, so a redraw shows on reload. The session keeps no page of its own.
- Once the person says to land it, the session approves it
  (`builds design approve N --by-person`) and lands with `land.sh --design N`.
  The change closes and leaves the tab. Changing the boards again
  (`builds design amend N`) clears the approval.
- The published canvas is the record of what landed.

## The boards are the target; the app follows them

- Where a board and the app differ, the app changes. A board's words, sizes,
  names (accessible names included), punctuation and layout are never copied
  from the app, its strings, its shots or a test harness's sample, not even to
  make a shot check pass or two platforms agree: the difference goes to the
  app's work queue.
- A board changes only when the person decides it, or when it breaks a written
  rule (the words check, the design system); the commit names which. Even then
  the new wording is designed, not taken from the app.
- Where a board has nothing (a placeholder, a control with no name), draw what
  the design calls for and say so; the app's version is not the answer by
  default.

## Colours come from one place

- On a platform, every colour an app shows comes from the design system: the
  accent the person chose, the surfaces, the text. None is left to the system's
  own colour (the Mac's accent, Material's dynamic colour) beside them. A mix
  clashes inside one screen, and two accents at once is never a design.
- When the platform draws a part in its own colour and offers no way to change
  it, the app draws that part itself or sets its colour; one that can't is a
  question for the person, never left native by default.

## The repo is the canvas's source

- The boards and canvas.json in the repo are the source; the published canvas
  and design system are copies. Edits made on the canvas page on claude.ai are
  accidents: a publish that finds the canvas saved from its page since the last
  one overwrites it with the repo's version and merges nothing from it.

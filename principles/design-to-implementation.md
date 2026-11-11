# Design to implementation

When a plan implements a design file:

- The plan carries the design's link and file name, and its block outline
  transcribed from the markup: nesting, order, sizes. Never a paraphrase; the
  outline proves the plan's author read the markup.
- The plan lists every change to the existing layout by name, so the implementer
  doesn't keep what the design replaced.
- The implementer and the reviewer open the design file itself before the plan's
  prose, and build and check against the markup.
- Done means the view's shot, drawn beside the board's render, matches it,
  before review and before merge.

The board is the preview; the code's picture of it is a shot. No IDE previews
(`#Preview`, Compose `@Preview`): they are slow, and a second picture of a
screen besides its board.

One name for each screen and each part:

- A board is named `<platform>.<area>.<screen>[.<state>]`, each part lower-case
  words joined by `-`: `mac.settings.subsonic.port-taken`,
  `android.welcome.join.name`. The platform comes first (`mac`, `android`,
  `ios`, `windows`, `linux`, `web`, or `brand` for what belongs to none); the
  area is where the screen lives in the product, not the canvas page it sits on;
  the state is left off for a screen's usual state.
- The shot of a board carries the board's name, as its id and its file name.
  Where code must name it with an identifier, that is the same words in
  PascalCase (`AndroidWelcomeJoinName`). Nothing is named twice: the name is
  derived, never chosen again.
- A design-system component and the code view that implements it have the same
  name (`MacFileNameField` in the design system and in the Swift code). A
  component's README names its view; a view no component names is either a
  component the design system is missing or code that should use one.

Shots confirm the build, and follow the boards:

- Every board of a screen the code builds has a shot: the app's real view, built
  in a test with staged data and drawn to an image, never by launching or
  driving the app. A board for something not built yet (an exploration, a
  proposal) has none.
- Checking a shot against its board is a program, not a reading: it compares
  what each one holds (each text and control, its place, size and colour) and
  reports what differs. Nobody, and no model, reads pictures to find differences
  the program can find.
- A record in the repository holds, for each confirmed pair, the board's and the
  shot's content hashes when they were compared and matched; a run checks only
  the pairs where either changed.
- The same program lists a board with no shot (to build), a shot whose board
  changed since it was confirmed (to update and confirm again), and a shot whose
  board is gone (to delete). Every design commit runs it, and the work it lists
  goes into the work item the commit files for the code. The hook refuses only
  the third: a shot of a deleted board is always wrong, while a board ahead of
  its shot is the normal state between a design landing and its code.

When drawing a design:

- Lay the screen out as `principles/drawing-screens.md` says: one thing per
  screen, the action under what it acts on, a wait that finishes on its own,
  parts rather than drawings.
- Start from what the code does today, then draw the screen the way the product
  should work. Anything the code can't do yet, already queued or not, is drawn
  anyway.
- A note beside the design names each thing the code needs for it, with the file
  and line of what it does today.
- The design system and the mocks live in the project's repository; the
  claude.ai pages are where they are seen and edited. Each change is published
  to the page and committed in the same step. An edit made on a page is copied
  back into the repository before the next change, so the repository is never
  more than one change behind.

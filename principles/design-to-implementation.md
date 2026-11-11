# Design to implementation

When a plan implements a design file:

- The plan carries the design's link and file name, and its block outline
  transcribed from the markup: nesting, order, sizes. Never a paraphrase; the
  outline proves the plan's author read the markup.
- The plan lists every change to the existing layout by name, so the implementer
  doesn't keep what the design replaced.
- The implementer and the reviewer open the design file itself before the plan's
  prose, and build and check against the markup.
- Done means the view's preview, rendered beside the design's render, matches
  it, before review and before merge.

One name for each screen and each part:

- A board is named `<platform>.<area>.<screen>[.<state>]`, each part lower-case
  words joined by `-`: `mac.settings.subsonic.port-taken`,
  `android.welcome.join.name`. The platform comes first (`mac`, `android`,
  `ios`, `windows`, `linux`, `web`, or `brand` for what belongs to none); the
  area is where the screen lives in the product, not the canvas page it sits on;
  the state is left off for a screen's usual state.
- The preview that renders the built view for a board carries the board's name:
  as written where a preview's id is a string (a macOS scene), in PascalCase
  where it must be an identifier (an Android `@Preview` function:
  `AndroidWelcomeJoinName`). Nothing is named twice: the name is derived, never
  chosen again.
- A design-system component and the code view that implements it have the same
  name (`MacFileNameField` in the design system and in the Swift code). A
  component's README names its view; a view no component names is either a
  component the design system is missing or code that should use one.

Previews confirm the build, and follow the boards:

- Every board of a screen the code builds has a preview, rendered from the app's
  own views and preview data, never by launching or driving the app. A board for
  something not built yet (an exploration, a proposal) has none.
- A record in the repository holds, for each confirmed pair, the board's content
  hash at the moment its preview was compared and found to match.
- One script compares boards, previews and that record and lists three things: a
  board with no preview (to build), a preview whose board changed since it was
  confirmed (to update and confirm again), and a preview whose board is gone (to
  delete). Every design commit runs it, and the work it lists goes into the work
  item the commit files for the code. The hook refuses only the third: a preview
  of a deleted board is always wrong, while a board ahead of its preview is the
  normal state between a design landing and its code.

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

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

Two kinds of shot, like unit and integration tests:

- **Component shots** draw each design-system component in each state its
  preview shows. The design system names a component `<Platform><Thing>`
  (`MacRow`, `AndroidSection`), the same `<Thing>` on every platform that has
  it. The code view that draws it has exactly that name and takes values the way
  the component takes props. Each named state in the component's preview is a
  shot: `<platform>.component.<thing>.<state>`, lower-case words joined by `-`
  (`MacRow`'s "Nested, off" is `mac.component.row.nested-off`).
- **Screen shots** draw whole screens, composed of components, as boards on the
  canvas draw them. A board is named `<platform>.<area>.<screen>[.<state>]`
  (`mac.settings.subsonic.port-taken`): platform first (`mac`, `android`, `ios`,
  `windows`, `linux`, `web`, or `brand` for what belongs to none), the area
  where the screen lives in the product, the state left off for its usual state.
  The screen's view is that name without the state, in PascalCase
  (`MacSettingsSubsonic`); its shot carries the full board name. Where two
  platforms have the same screen, `<area>.<screen>` is the same on both. Each
  board in the canvas index says whether it gets a shot: the person picks which
  boards are covered.
- Every name follows from a component, a preview state or a board; none is
  chosen twice. A view that only reads app state to feed one component has no
  reason to exist: the screen reads the state and places the component. Where
  one must exist, it is `<Component>Container`.

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

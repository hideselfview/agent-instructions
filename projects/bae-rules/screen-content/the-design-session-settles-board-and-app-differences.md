---
paths:
  - design/**
  - plans/*.md
  - plans/briefs/**
blocking: true
design: true
---

## The design session settles board and app differences

Each app builds its screens to match their boards
(`build-each-screen-to-match-its-board.md`). When a board and the app differ,
the design session judges which of the two draws the screen as it should be, and
brings the other into agreement with no question to the person and no wait for
their approval (decision #1140 in project bae). The person reviews what lands.

- A board that draws a change the person decided is right, and the app follows
  it.
- When the board is right, the design session puts the app's change on the work
  queue.
- When the app is right and the person decided nothing about the difference, the
  design session redraws the board to draw what the app draws, and records the
  redraw as a fix (`builds help workflow`, DESIGN CHANGES).
- When neither draws the screen as it should be, and the person's decisions, the
  spec or the platform's conventions settle what it should be, the design
  session draws it on the board, records it as a fix and lands it, with no
  review by the person (decision #1422 in project bae). For example, an alert
  whose button label does not fit side by side stacks its buttons, as macOS
  does. Only a change that nothing settles goes to the person.
- A redrawn board follows the design system's guidelines like any other board
  (`design/system/README.md` lists them).

A board changes only in these cases, and the commit says which:

- The person decided a change.
- The design session judged that the app draws the screen as it should be, and
  the person decided nothing about the difference.
- The design session judged that neither draws the screen as it should be, and
  the person's decisions, the spec or the platform's conventions settle what it
  should be.
- The board breaks a written rule (the words check, the design system's
  guidelines).

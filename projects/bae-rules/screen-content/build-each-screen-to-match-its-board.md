---
digest: 'A board of a screen the app already has draws it as the app draws it today unless the person decided a change to it, and copying the app there needs no other decision (decision #992); a board of a decided change or a new screen leads, the app follows it, and the board never copies the app. For a change the person decided to an existing screen that existing design-system parts can draw, the implementing agent edits the board in the same commit and names the decision; the design session draws new screens, new layouts and new or changed parts. A plan that builds a board transcribes its markup; every built board has a shot of the real view, named from its board or component and checked by a program, never by reading pictures. Screens show one thing, put the action under what it acts on, wait without a button, use design-system parts, and take every colour from the design system.'
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
  - plans/**
blocking: true
---

## Build each screen to match its board

Each screen has a board, and each app builds the screen to match it. Whether the
board or the app leads depends on what the person decided (decision #992 in
bae's decision log):

- A board of a screen the app already has draws the screen as the app draws it
  today, unless the person decided a change to it.
  - The board takes the app's words, sizes, names, punctuation and layout.
  - A board that copies the app this way needs no other decision.
- A board of a change the person decided, or of a new screen, leads, and the app
  follows it.
  - Where the board and the app differ, the app changes: put the difference on
    the app's work queue.
  - Never copy the board's words, sizes, names (accessible names included),
    punctuation or layout from the app, its strings, its shots or a test's
    sample data, not even to make a shot check pass or two platforms agree.

Where a board leaves something out (a placeholder, a control with no name), draw
what the design calls for and say so.

A board changes only in these cases, and the commit says which:

- The person decided a change.
- The board breaks a written rule (the words check, the design system).
- The board draws a screen the app already has differently from how the app
  draws it today, and the person decided nothing about the difference.

When the person decides a change to an existing screen that the design system's
existing parts can draw, such as new words or a button added to an alert, the
implementing agent edits the board in the same commit and names the decision.
The design session draws new screens, new layouts, and anything that needs a new
part or a change to the design system.

### Laying out a screen

- Show one thing per screen.
  - The headline says what to do or what happened ("Tell the person who invited
    you this code", "This invite has ended").
  - A screen that needs a second sentence to explain itself is two things. Cut
    one, or move it where it applies: a question about a field goes in the
    field's helper line.
  - The bar or window title says where you are ("Join library"). Neither repeats
    the other.
- Put each button right under the field, code or line it acts on, a section
  apart.
  - Never pin it to the bottom across an empty gap.
  - A form or an outcome starts at the top and reads down, left-aligned.
  - Centre a screen only when it shows one thing to look at, such as a code to
    read aloud.
- A screen that waits on something outside it (another device, another person)
  checks by itself every few seconds while it is on screen.
  - It shows a spinner beside a line that says what it waits for ("Waiting for
    your other device…").
  - It has no countdown, no "Check now" button and no deadline the person can do
    nothing about.
  - When the wait ends, the next screen says so and what to do.
- Anything that appears on more than one screen (a code, a step's layout, a row
  of buttons) is a design-system part, used everywhere it appears.
  - Drawing it by hand on a board is a bug, even when the pixels match.
  - Before you add a part, look for one the design system has, and use or extend
    it.
- Every colour an app shows comes from the design system: the accent the person
  chose, the surfaces, the text.
  - None comes from the platform (the Mac's accent, Material's dynamic colour).
  - When the platform draws a part in its own colour and offers no way to change
    it, the app draws that part itself.
  - When the app can't, ask the person.

### Drawing a board

Start from what the code does today. For a screen the app already has, draw what
the code draws. Where the person decided a change, and for a new screen, draw
the screen the way the product should work, including what the code can't do
yet. In a note beside the board, name each thing the code needs for it, with the
file and line of what the code does today.

### Building a board

A plan that builds a board gives the board's link and file name, and its block
outline copied from the markup (nesting, order, sizes), never a paraphrase. It
lists by name every change to the existing layout, so the implementer doesn't
keep what the board replaced. The implementer and the reviewer open the board's
file before the plan's prose, and build and check against the markup.

A shot draws the code's version of a board, so don't add IDE previews
(`#Preview`, Compose `@Preview`). There are two kinds of shot:

- A component shot draws a design-system component in each state its preview
  shows.
  - A component is named `<Platform><Thing>` (`MacRow`, `AndroidSection`), with
    the same `<Thing>` on every platform that has it.
  - The view that draws it has exactly that name and takes values the way the
    component takes props.
  - Each named preview state is a shot named
    `<platform>.component.<thing>.<state>`, in lower-case words joined by `-`:
    `MacRow`'s "Nested, off" is `mac.component.row.nested-off`.
- A screen shot draws a whole screen, made of components, as its board does.
  - A board is named `<platform>.<area>.<screen>[.<state>]`
    (`mac.settings.subsonic.port-taken`).
    - The platform is `mac`, `android`, `ios`, `windows`, `linux`, `web`, or
      `brand` for none.
    - The area is where the screen is in the product.
    - The state is left off for the usual one.
  - The screen's view is that name without the state, in PascalCase
    (`MacSettingsSubsonic`), and its shot has the full board name.
  - A screen on two platforms has the same `<area>.<screen>` on both.
  - The canvas index says which boards get a shot; the person picks them.

Every name follows from a component, a preview state or a board. The view that
reads app state for a component is `<Component>Container`
(`MacNowPlayingBarContainer`), nested just above the component, because a view
that reads a piece of state redraws when it changes
(`rules/ui-state/a-component-reads-its-state-at-the-leaf.md`). A screen's view
reads only what no container below it reads. Pass a value down only when it
rarely changes; in Compose, pass a lambda (`{ position }`) so the read happens
where the value is used. The component under the container takes plain values,
so its shot can draw it from a preview state with no app running.

### Checking shots against boards

- Every board of a screen the code builds has a shot.
  - A shot is the app's real view, built in a test with staged data and drawn to
    an image, never by launching or driving the app.
  - A board for something not built yet has none.
- A view is done when its shot matches its board, before review.
- A program compares a shot with its board: each text and control, its place,
  size and colour.
  - Nobody, and no model, reads pictures to find what the program can find.
- The same program lists what needs work:
  - a board with no shot
  - a shot whose board changed since it last matched
  - a shot whose board is gone
- Every design commit runs the program, and what it lists goes into the design
  change the commit records (`builds design record`).
- The hook refuses only a shot whose board is gone. A board ahead of its shot is
  normal between a design landing and its code.

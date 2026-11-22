---
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
blocking: true
design: true
---

## No restyling a component from outside

Each component's look comes from the component and the props it is given. A
container never changes how a child component looks. On the canvas that means no
selector such as `.bae-m-actlist .bae-notice { background: none }` or
`.bae-notice .bae-m-btn-secondary { … }`; in the apps, no parent modifier or
environment value that repaints a child component.

When a component has to look different in some place (a quiet notice inside the
Activity panel, a button on a dark stage), add a variant to the component (a
prop such as `quiet`, with its own styles in the component), and have that place
pass it. A reader then sees the look where the component is used, and the
component's preview can show every look it has.

A component may style its own parts. The container owns the layout around a
component (spacing, position); the component owns how it looks.

A component also sets everything about itself that it would otherwise inherit,
above all its text alignment. A text field declares `text-align: start` (or its
platform's leading alignment) itself, so a centred screen around it can't move
its helper line to the middle. When a container's layout changes a child's look,
the fix goes in the child, never in a container rule that undoes it.

The same goes for a component's padding: when it is wrong in some place, the
padding belongs to the places that want it (their gap) or to a variant, never to
a rule such as `.bae-m-step > .bae-m-code { padding: 0 }`.

---
digest: A component looks and aligns the way its own props say. Never restyle it from where it sits (a parent's CSS reaching into it); when it must look different there, give it a variant. A component sets its own text alignment, and anything drawn on two screens is a component.
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
blocking: true
---

## No restyling a component from outside

We're building a design system: each component's look comes from the component
and the props it's given. A container never reaches in and changes how a child
component looks. On the canvas that means no selector like
`.bae-m-actlist .bae-notice { background: none }` or
`.bae-notice .bae-m-btn-secondary { … }`; in the apps, no parent modifier or
environment value that quietly repaints a child component.

When a component has to look different in some place (a quiet notice inside the
Activity panel, a button on a dark stage), add a variant to the component (a
prop such as `quiet`, with its own styles in the component) and have that place
pass it. A reader then sees the look where the component is used, and the
component's preview can show every look it has.

A component may style its own parts. Layout around a component (spacing, where
it sits) belongs to the container; how the component itself looks does not.

A component also sets everything about itself that it would otherwise inherit:
its text alignment above all. A text field declares `text-align: start` (or its
platform's leading alignment) itself, so a centred screen around it can't pull
its helper line to the middle. When a container's layout leaks into a child's
look, the fix goes in the child, never a container rule that undoes it.

The same goes for the space a component carries: if it has padding that's wrong
in some place, the padding belongs to the places that want it (their gap), or to
a variant, not to a rule like `.bae-m-step > .bae-m-code { padding: 0 }`.

**Anything drawn on two screens is a component.** A code shown in a dialog and
on a waiting screen, a step's headline-and-action layout, a row of buttons: each
is one part in the design system, used everywhere, so the screens can't drift. A
board that hand-draws one (an inline-styled `<div>` with the digits, a `flex: 1`
spacer pushing a button down) is the bug even when it looks right; replace it
with the part, or make the part.

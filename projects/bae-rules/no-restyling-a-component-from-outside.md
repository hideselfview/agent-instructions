---
digest: A component looks the way its own props say. Never restyle it from where it sits (a parent's CSS reaching into it); when it must look different there, give it a variant.
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

---
digest: Every user-facing capability ships on both visible-ios and visible-android against the same bridge calls; a feature on one platform without the other is incomplete.
paths:
  - visible-ios/**
  - visible-android/**
blocking: false
---

## iOS and Android are at parity

Every user-facing capability ships on both `visible-ios` and `visible-android`,
backed by the same `visible-bridge` calls. When a change adds, removes, or
alters a user-facing behavior on one platform — a screen, an action (add /
rename / delete / move a node, take or replace a photo), a navigation
affordance, an empty/error state — the matching change must land on the other
platform in the same PR. Flag a diff that touches only `visible-ios/**` or only
`visible-android/**` with a user-facing behavior change and no counterpart on
the other side.

Not a violation: platform-mechanism code with no user-facing behavior of its own
(camera permission plumbing, lifecycle wiring, build config), which differs by
platform by nature.

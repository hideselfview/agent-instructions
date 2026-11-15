---
digest: Every user-facing capability ships on both visible-ios and visible-android against the same bridge calls; a feature on one platform without the other is incomplete.
paths:
  - visible-ios/**
  - visible-android/**
blocking: false
---

## iOS and Android are at parity

Every user-facing capability ships on both `visible-ios` and `visible-android`,
backed by the same `visible-bridge` calls. When a change adds, removes or alters
a user-facing behavior on one platform, the matching change lands on the other
in the same commit. A user-facing behavior is, for example, a screen, an action
(add, rename, delete or move a node, take or replace a photo), a way to
navigate, or an empty or error state. Flag a diff that changes user-facing
behavior in only `visible-ios/**` or only `visible-android/**`.

Code with no user-facing behavior of its own (camera permission handling,
lifecycle setup, build config) differs by platform and is not a violation.

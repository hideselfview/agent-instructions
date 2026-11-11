---
digest: An Android shot or Compose UI test holds the compose clock (`mainClock.autoAdvance = false`) and advances it a fixed amount before it captures, so an animation is caught at the same frame every run and an endless one can't keep the test waiting.
paths:
  - bae-android/**
blocking: true
---

## Android shots hold the clock

A compose rule's clock advances on its own by default: each idle wait runs
frames until nothing is animating. A spinner, the welcome logo's bob and glow or
a progress bar following a position animates without end, so a capture lands on
whatever frame the run reached (a different picture each run), and a wait for
idle on an endless animation never returns.

So a Roborazzi shot, and a Compose UI test that reads layout or pixels, turns
the clock off and moves it itself:

```kotlin
compose.mainClock.autoAdvance = false
compose.setContent { ShotTheme { Screen(state) } }
compose.mainClock.advanceTimeBy(SHOT_SETTLE_MS) // the same amount on every run
compose.onRoot().captureRoboImage(file)
```

The shot harness does this once for every shot; a shot doesn't do it by hand.

`BaeRobolectricTestRunner` paces frames by the looper's clock for tests without
a compose rule, and leaves compose-rule tests to their own clock (its doc says
why), so this rule is what keeps compose-rule tests steady.

The macOS counterpart is `windows-in-tests-never-animate.md`.

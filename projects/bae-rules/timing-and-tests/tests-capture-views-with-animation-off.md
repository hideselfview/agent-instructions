---
paths:
  - bae-macos/**
  - bae-android/**
blocking: true
capture: true
---

## Tests capture views with animation off

A test or shot that captures a view, or reads its layout, while something on
screen animates gets a different picture each run, so it fails some runs and
passes others. An animation that never ends (a spinner, the welcome logo's bob
and glow, a progress bar that follows a position) can also keep the test waiting
forever. So every test and shot turns animation off before it captures, in every
code path, and never waits an animation out with a delay.

### macOS

macOS plays a zoom-in when a window is shown: the window starts at about 80% of
its size and grows to full over a few frames. Off every display, where baeShots
puts its content windows, no display paces the animation, and it never ends.

Every `NSWindow` a test or shot creates sets `animationBehavior = .none` before
`orderFront`, `makeKeyAndOrderFront` or `makeKey`. That holds for a borderless
content window and a titled whole-window capture alike:

```swift
let window = ShotWindow(contentRect: bounds, styleMask: [.titled], backing: .buffered, defer: false)
window.animationBehavior = .none   // before it is shown
window.contentViewController = hosting
window.makeKeyAndOrderFront(nil)
```

A SwiftUI transition or `withAnimation` between showing a view and capturing it
is turned off for the capture (`.transaction { $0.disablesAnimations = true }`).

### Android

A compose rule's clock advances by itself by default: each wait for idle runs
frames until nothing animates. A capture then shows whatever frame the run
reached, and a wait for idle on an endless animation never returns.

So a Roborazzi shot, and a Compose UI test that reads layout or pixels, turns
the clock off and advances it by the same amount on every run:

```kotlin
compose.mainClock.autoAdvance = false
compose.setContent { ShotTheme { Screen(state) } }
compose.mainClock.advanceTimeBy(SETTLE_MILLIS)
compose.onRoot().captureRoboImage(file)
```

The shot harness (`settle` in
`bae-android/app/src/test/java/fm/bae/app/shots/BoardShot.kt`) does this for
every shot, so a shot never does it by hand.

`BaeRobolectricTestRunner` paces frames by the looper's clock for a test without
a compose rule, and leaves a compose-rule test to the rule's clock, so this rule
is what keeps compose-rule tests steady.

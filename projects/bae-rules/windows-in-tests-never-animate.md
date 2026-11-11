---
digest: Every window a macOS test or shot opens sets `animationBehavior = .none` before it is ordered front, so its zoom-in never plays and nothing is captured mid-animation.
paths:
  - bae-macos/**
blocking: true
---

## Windows in tests never animate

macOS plays a zoom-in when a window is ordered front: it starts at about 80% of
its size and grows to full over a few frames. A test or baeShots scene that
captures the window, or reads its layout, during those frames gets a different
picture each run, so it fails some runs and passes others. Off every display
(where baeShots puts its content windows) the animation has no display to pace
it and never ends.

So every `NSWindow` a test or shot creates sets `animationBehavior = .none`
before `orderFront`, `makeKeyAndOrderFront` or `makeKey`, in every code path: a
borderless content window and a titled whole-window capture alike.

```swift
let window = ShotWindow(contentRect: bounds, styleMask: [.titled], backing: .buffered, defer: false)
window.animationBehavior = .none   // before it is shown
window.contentViewController = hosting
window.makeKeyAndOrderFront(nil)
```

The same goes for any other motion between showing a view and capturing it: a
SwiftUI transition or `withAnimation` is turned off for the capture
(`.transaction { $0.disablesAnimations = true }`), never waited out with a
delay.

Seen 2026-11-11: whole Settings window shots opened titled windows without it
and were captured from the window server while still zooming in. It had caused
flaky shots before, and the content path already set it (ShotRenderer.swift).

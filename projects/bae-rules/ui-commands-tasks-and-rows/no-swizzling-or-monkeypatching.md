---
paths:
  - bae-macos/**
  - bae-ios/**
  - bae-android/**
  - bae-avalonia/**
blocking: true
---

## No swizzling or monkeypatching of system methods

An app never replaces or reaches inside a system framework at runtime: no
Objective-C method exchange (`method_exchangeImplementations`,
`class_replaceMethod`, `method_setImplementation`), no reflection into a
framework's private fields or methods on Android or .NET, and no undocumented
user default set to change how a framework behaves. A replaced method can stop
working, or start crashing, with any OS update, and nothing says so until it
does.

When a framework misbehaves, fix it with its documented API: change how bae
declares what the framework handles, or move to the layer whose documented
behavior fits. bae's macOS menus, for example, are an AppKit shell (an
`NSApplicationDelegate` and a plain `NSMenu` of AppKit's standard Edit items),
because SwiftUI's App lifecycle crashed in the Edit menu.

The Mac shot tool (`bae-macos/bae/baeShots`) is test code that never ships, and
it may use CoreGraphics' private `CGVirtualDisplay` classes to draw window
scenes on a virtual display larger than the Mac's own: macOS has no documented
way to make one, and CI's Mac screen is smaller than some of bae's windows. The
person decided this. Everything else in the shot tool, and all of the app,
follows this rule.

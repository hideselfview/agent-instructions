---
digest: The apps never swizzle or monkeypatch a system framework's methods (Objective-C method exchange, reflection into a framework's private members, undocumented defaults); a framework fault is fixed through its documented API.
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

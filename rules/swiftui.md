---
paths:
  - "**/*.swift"
---

# SwiftUI rules

## Read at the leaf, not the parent

Pass the observable object (store, appService) down. Let the leaf view read
the specific field it needs. Don't read in a parent and pass the value —
that subscribes the parent to changes it doesn't care about. SwiftUI tracks
which view accessed which `@Observable` property; reading in a parent widens
the re-render scope unnecessarily.

## `@Environment` and `@Binding` are mutually exclusive paths to the same state

If a child reads `@Environment(UiState.self)` and also declares `@Binding`
to a field on that uiState, the binding is pure duplication — pick one
path. Default fix: remove the env read (make the child fully pure with
bindings), not the bindings — env-driven children force previews to
construct a real `UiState`, which wrecks preview ergonomics.

Legitimate reasons to take a `@Binding`:

- A framework API demands it (`NavigationStack.path`, `Picker.selection`,
  `TextField.text`, `Toggle.isOn`, `Slider.value`, etc.).
- State lives in a parent's `@State` and isn't reachable from
  `@Environment` — binding is the only path.
- The child is intentionally pure / prop-driven — doesn't read
  `@Environment(UiState.self)` at all. Previews and reuse rely on this.

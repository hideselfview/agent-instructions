---
digest: A change to @Observable state that reads the current value and writes a new one is a mutating method on the state type, called once from the view, never a read and a write in a view's action handler.
paths:
  - '**/*.swift'
blocking: false
---

## Change observable state in its own methods

When a view action changes `@Observable` state based on its current value, make
that read and write one `mutating func` on the state type, and call it from the
view.

A view's computed property that reads `@Observable` state keeps its read access
open while SwiftUI renders. When SwiftUI delivers an event during rendering (a
key press, a tap), an action handler that reads the same property and writes the
state back overlaps that read, and the app crashes with an exclusivity
violation.

`uiStore.lightbox?.goToNext()` is one access. Reading the cursor's current
position in the view and then writing `uiStore.lightbox` is two overlapping
accesses.

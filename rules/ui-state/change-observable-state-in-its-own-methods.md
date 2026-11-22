---
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

A view that shows the value the person changes, such as a checkbox or a
checkmark, sends the value it wants to the state type's setter instead
(`set-state-don-t-toggle`). The mutating method is for a change the person makes
without choosing a value, such as Next Image or a Toggle Queue menu item with no
checkmark (`uiStore.toggleSidePane()`), and it is the only place the code flips
or steps that state.

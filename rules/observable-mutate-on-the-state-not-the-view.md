---
digest: Read-modify-write of @Observable state belongs in a mutating func on the state, not the view.
paths:
  - '**/*.swift'
blocking: false
---

## @Observable: mutate on the state, not the view

Computed properties on a SwiftUI view that read `@Observable` state, combined
with action handlers that read the same state and write back, cause runtime
exclusivity crashes. When SwiftUI dispatches an event during rendering (key
press, button tap), the read access from rendering is still open — the handler's
read+write triggers a simultaneous access violation.

Banned: view computed properties that read `@Observable` state + action handlers
that read those properties and write back to the same state. The read from
rendering and the write from the handler overlap.

Fix: mutation logic belongs on the state type, not the view.
`uiState.lightbox?.navigateNext()` is one access. Reading `currentIndex` then
writing `uiState.lightbox?.currentIndex` is two overlapping accesses.

Rule of thumb: if a view action mutates `@Observable` state based on its current
value, that read-modify-write must be a single `mutating func` on the state
type.

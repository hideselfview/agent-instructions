## State describes what is, not what should happen

Don't use observable state fields as imperative command triggers. The "set a
flag, observe via `.task(id:)`/`onChange`, reset the flag" pattern conflates two
concepts: state is what the UI *is*; commands are what should *happen once*.
Packing a one-shot command into an observable field leaks it through every
reader, requires edge-triggered guards, and creates subtle bugs (equal-value
sets don't re-fire).

For ephemeral commands (scroll, flash, focus, play-this-once), use pub/sub:

- **SwiftUI**: `PassthroughSubject` / `AsyncStream` — sender publishes, receiver
  subscribes via `.onReceive`.
- **React**: event emitter or imperative ref method.
- **Dioxus**: channel or explicit signal dispatch.

Diagnostic: if you find yourself writing `uiState.xyz = value` followed by a
reader that resets `uiState.xyz = nil` after handling, the design is wrong —
replace with a subject.

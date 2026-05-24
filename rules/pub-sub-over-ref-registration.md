---
digest: A→B imperative push uses pub/sub (B subscribes), not ref-registration through containers.
paths:
  - '**/*.swift'
  - '**/*.tsx'
  - '**/*.jsx'
  - bae-web/**/*.rs
blocking: false
---

## Pub/sub over ref-registration

When component A needs to push imperative updates to component B — bypassing the
normal reactive flow — prefer pub/sub: B subscribes to a stream A publishes to.
Avoid ref-registration, where intermediate containers collect refs to B and the
parent calls methods on them.

Registration forces every layer in the hierarchy to know B's type and wire the
connection, leaking B through containers that shouldn't care about it. Pub/sub
lets B self-wire at construction — no registration step, no leaked types, no
coupling through the tree.

Framework instances:

- **SwiftUI**: `PassthroughSubject` / `AsyncStream` — A publishes, B subscribes
  via `.onReceive` or a `.task` await loop.
- **React**: event emitter (`EventTarget`, mitt, RxJS subject) or a context with
  a callback registry.
- **Dioxus**: channel (`futures::channel::mpsc`) or a signal B watches.

Fall back to registration only when the update requires calling multiple methods
on a stateful object that can't be captured in a single message.

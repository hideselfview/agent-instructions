---
digest: In any UI framework, observable state says what the UI is; a one-shot command (scroll, flash, focus, play once) travels on a stream the receiver subscribes to, not a flag that is set, observed and reset; and a component that pushes commands to another publishes to a stream the other subscribes to, rather than registering references through containers.
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.tsx'
blocking: false
---

## State describes what is, not what should happen

This rule holds in every UI framework. Observable state says what the UI is; a
command says what should happen once. Don't use an observable field to trigger a
command: setting a flag, reacting to its change, then resetting it. A command
stored as state reaches every reader of the state, needs guards that fire only
on a change, and fails when the same value is set twice, because nothing
changed.

- SwiftUI: a flag observed with `.task(id:)` or `.onChange`.
- Compose: a flag used as the key of a `LaunchedEffect`.

Send one-shot commands (scroll, flash, focus, play once) on a stream instead:
the sender publishes, and the receiver subscribes.

- SwiftUI: a `PassthroughSubject` received with `.onReceive`, or an
  `AsyncStream` read in `.task`.
- Compose: a `Channel` or `MutableSharedFlow` collected in a `LaunchedEffect`.

When you write `uiStore.xyz = value`, and the reader sets `uiStore.xyz = nil`
after handling it, replace the field with a stream.

### Streams, not registered references

When component A pushes commands to component B outside the normal flow of
state, B subscribes to a stream A publishes to. Don't have the containers
between them collect references to B for A to call. Registration makes every
layer between them know B's type and wire the connection; with a stream, B
subscribes itself when it is created, and no layer in between knows about it.

Register a reference only when one update has to call several methods on a
stateful object, and no single message can carry it.

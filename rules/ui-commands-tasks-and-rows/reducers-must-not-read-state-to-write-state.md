---
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.tsx'
blocking: false
---

## Reducers must not read state to write state

A reducer, the handler that writes an event into the store, computes what it
writes from the event alone, in any UI framework. A reducer that looks up
another part of the state (`summaries[albumId]`) to compute what it writes
(`storageSummaries[releaseId]`) makes the store depend on order, so whichever
part filled first wins, and it needs a default for when the lookup misses.

Widen the event's payload instead, so the reducer is a function of the event
alone. An event may carry the ids of other records, but the reducer must not
need to look them up. The producer (the emitter, the bridge, the backend) may
have to join data before it emits, so the payload holds what every reducer needs
rather than the raw database row.

A UI action that reads the value it shows to compute a command's argument
(`set(!shuffled)`) is not a reducer, and is allowed (`set-state-don-t-toggle`).

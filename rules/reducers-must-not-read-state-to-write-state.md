---
digest: Reducers derive output purely from the event payload; widen the payload, don't dereference other slices.
paths:
  - '**/*.swift'
  - '**/*.tsx'
  - '**/*.jsx'
  - bae-web/**/*.rs
blocking: false
---

## Reducers must not read state to write state

Event handlers that mutate state must derive their output purely from the event
payload. If a reducer reaches into other state slices (`summaries[albumId]`) to
compute what it writes (`storageSummaries[releaseId]`), the store becomes
ordering-dependent — whichever slice populated first wins — and forces fallback
defaults on cache miss.

Fix: widen the event payload so the reducer is a pure function of the event.
Events can carry foreign-key ids, but must not require the reducer to
dereference them against other slices. On the producer side (emitter, bridge,
backend), this may mean joining data before emitting so the event payload
mirrors what every reducer consumer needs — not the raw DB row.

Scope: this is about *reducers* — handlers that write to the store. A UI action
that reads the value it's rendering to compute a command's argument
(`set(!shuffled)`) is not a reducer and not a violation — that's the preferred
absolute-`set` shape (`rules/set-state-don-t-toggle.md`), not a store write
dereferencing another slice.

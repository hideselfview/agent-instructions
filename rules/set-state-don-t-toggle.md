---
digest: A command carries the state it wants - set(on), setMode(x) - never "flip the current one" (toggle, cycle). A toggle isn't idempotent, so a retry, a double tap or a stale or duplicate dispatch ends in the wrong state. The caller may compute the target from what it shows (set(!shuffled)) and send that value down, even across a bridge to a core that holds the state.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

## Set the state you want; don't toggle

A command that changes a value carries the value it wants: `set(on: true)`,
`setMode(.context)`. It never says "flip whatever is there": `toggle()`,
`cycle()`, "advance from the current state".

A toggle isn't idempotent: run it twice and it undoes itself. So every way a
command can run more than once, or run against a stale view, ends in the wrong
state: a double tap, a retry after a lost reply, a duplicated event, two windows
driving the same value. A `set` ends in the same place however many times it
runs.

The caller computes the target from what it shows (`set(!shuffled)`,
`setRepeat(mode.next())`) and sends that value down to the core, the store or
the bridge, even when the core also holds the state. A `!current` that names the
target is not domain logic, so it doesn't break
`ui-iterates-and-renders-nothing-more` or
`reducers-must-not-read-state-to-write-state`.

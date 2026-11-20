---
digest: A command carries the value it wants, such as set(on) or setMode(x), never "flip the current one" (toggle, cycle), because a toggle isn't idempotent: a retry, a double tap or a stale or duplicate dispatch ends in the wrong state. A caller that shows the value computes the target from it (set(!shuffled)) and sends that value down, even across a bridge to a core that holds the state. A caller that shows no value, such as a Toggle Queue menu item with no checkmark, calls one method on the type that holds the state, which flips that state itself (toggleSidePane()). That method is the only toggle the code holds, and every command to a core, a bridge or another store carries a value.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
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

A caller that shows the value computes the target from what it shows
(`set(!shuffled)`, `setRepeat(mode.next())`) and sends that value down to the
core, the store or the bridge, even when the core also holds the state. A
`!current` that names the target is not domain logic, so it doesn't break
`ui-iterates-and-renders-nothing-more` or
`reducers-must-not-read-state-to-write-state`.

### When the caller shows no value

A menu item with no checkmark (Toggle Queue), a keyboard shortcut or a button
whose label never changes shows the person no value to choose, so the person's
command means "flip whatever is there". A step such as Next Image means "move
from wherever it is" in the same way. Such a caller has no value of its own to
send. A `set(!store.showSidePane)` that it builds reads the live state at
dispatch, so a double dispatch flips twice, as a toggle does, and in SwiftUI the
read and the write overlap (`change-observable-state-in-its-own-methods`).

So the change is one method on the type that holds the state, which reads and
writes that state in one place (`uiStore.toggleSidePane()`,
`lightbox.goToNext()`). That method is the only toggle the code holds:

- A caller that shows the value sends the value it wants, even when the type has
  a toggle method.
- A toggle method flips state that its own type holds. Every command to a core,
  a bridge or another store carries a value.

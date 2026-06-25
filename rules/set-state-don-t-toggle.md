---
digest: Commands carry the target state — set(on)/setMode(x) — never "flip the current one" (toggle/cycle). Toggles aren't idempotent, so a retry, double-fire, or stale/duplicate dispatch lands in the wrong state. The caller may compute the target from what it renders (set(!shuffled)) and send that absolute value down — including across a bridge the core also holds — that's correct, not a thinness/derive violation.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

## Set the state you want, don't toggle

A command that changes a value carries the value it wants — `set(on: true)`,
`setMode(.context)`. It never says "flip whatever's there" (`toggle()`,
`cycle()`, "advance from the current state").

A toggle isn't idempotent — run it twice and you've undone it. So every way a
command can fire more than once, or fire against a stale view, lands in the
wrong state: a double tap, a retry after a dropped reply, a duplicated event,
two windows driving the same value. A `set` lands in the same place however many
times it runs.

The caller computes the target from what it's currently showing —
`set(!shuffled)`, `setRepeat(mode.next())` — and sends that absolute value down
to the core/store/bridge. This is the correct, preferred shape. It is **not** a
"UI derives domain state", "reducer reads state to write state", or "bridge
thinness" violation, and sending the value across a bridge the core also holds
is fine — a one-line `!current` that names the target is not domain logic.
(Carve-outs are noted in those rules.)

Pairs with `rules/state-describes-what-is-not-what-should-happen.md`: state is
what *is*; a command names the state it wants, not a verb against the current
one.

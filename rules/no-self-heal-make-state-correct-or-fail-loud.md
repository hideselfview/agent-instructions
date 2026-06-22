---
digest: Never leave durable state wrong and trust a later pass to repair it (self-heal / reconcile-later / converge-next-cycle); make the bad state unrepresentable, or make the operation atomic — commit-whole-or-roll-back, else fail loudly to its initiator who retries it idempotently.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: true
---

**No self-heal — make the broken state impossible, or fail loud.**

When a step in a multi-step operation can fail, never leave durable state wrong
and trust a later pass to notice and repair it. "Reconcile next cycle," "a
background sweep fixes it," "it converges eventually," "self-heal," "it catches
up" — all the same anti-pattern. It is wrong on three axes: a window of wrong
state nobody bounded (a removed member who still has access, a published row
whose blob never uploaded, a cursor advanced past data never seen); a failure
hidden from the one party who could have acted on it (the operation reported
success); and a reconciler you now also have to keep correct, run, and trust —
new surface that exists only to paper over the first mistake.

Only two shapes are acceptable:

1. **Make the bad state unrepresentable — correct by construction.** The
   contradiction can't be written down, so there is nothing to repair. (An
   outbox that holds at most one operation per key, decided at enqueue; a type
   that can't encode the invalid combination; "durable position — a cursor, a
   head, a commit — advances only over fully-realized, verified work," so a
   half-done step never moves the marker.)

2. **Atomic, or fail loud to the initiator.** The operation commits as a whole
   or rolls back; if a sub-step fails, the whole operation fails loudly to
   whoever invoked it, and they retry the *whole* operation. Make it idempotent
   so retry is safe. The state is never left wrong on the bet that something
   else will fix it.

The tell that you're reaching for self-heal: you've written the happy path, a
sub-step can fail, and your instinct is to log-and-continue / return success
anyway / add a background job that "reconciles." Stop. Either the broken state
shouldn't be representable, or this call must fail and be retried as a unit.

```rust
// Bad — sub-step fails, operation reports success, "a later cycle reconciles"
pub async fn remove_member(...) -> Result<Key> {
    let key = revoke_and_rotate(...).await?;
    if let Err(e) = sync_authorized_keys(...).await {
        warn!("failed to sync authorized keys: {e}");   // removed member keeps access
    }
    Ok(key)                                              // caller believes it's done
}

// Good — atomic-or-fail: the whole removal completes or the caller retries it
pub async fn remove_member(...) -> Result<Key> {
    let key = revoke_and_rotate(...).await?;
    sync_authorized_keys(...).await?;   // its failure fails the removal
    Ok(key)
}
```

Pairs with `never-mask-errors-with-defaults.md`: that rule bans swallowing the
error at the point it occurs; this one bans tolerating the wrong *state* the
swallowed error leaves behind, and bans the background "reconciler" proposed to
clean it up.

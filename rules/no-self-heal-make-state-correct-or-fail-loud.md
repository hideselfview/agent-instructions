---
digest: Never leave durable state wrong and trust a later pass to repair it (self-heal, reconcile later, converge next cycle); make the bad state impossible to represent, or make the operation atomic - it commits whole or rolls back, else fails loudly to its caller, who retries it idempotently.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: true
---

## No self-heal: make the broken state impossible, or fail loudly

When a step in a multi-step operation can fail, never leave durable state wrong
and trust a later pass to notice and repair it. "Reconcile next cycle", "a
background sweep fixes it", "it converges eventually", "self-heal" and "it
catches up" all describe this. Each leaves state wrong for a time nobody bounds
(a removed member who still has access, a published row whose file never
uploaded), hides the failure from the caller who could act on it, and adds a
reconciler that must itself be kept correct.

Two shapes are allowed:

1. **Make the bad state impossible to represent.** The contradiction can't be
   written down, so there is nothing to repair.
   - An outbox holds at most one operation per key, decided when the operation
     is queued.
   - A type can't encode the invalid combination.
   - A durable position (a cursor, a head, a commit) advances only over work
     that is complete and verified, so a half-done step never moves it.
2. **Make the operation atomic, or fail loudly to its caller.** The operation
   commits whole or rolls back. When a sub-step fails, the whole operation fails
   to its caller, who retries the whole operation; make it idempotent so the
   retry is safe.

You are reaching for self-heal when you have written the happy path, a sub-step
can fail, and you want to log and continue, return success anyway, or add a
background job that reconciles. Either the broken state must not be
representable, or this call must fail and be retried as a unit.

```rust
// Bad: a sub-step fails, the operation reports success, a later cycle reconciles
pub async fn remove_member(...) -> Result<Key> {
    let key = revoke_and_rotate(...).await?;
    if let Err(e) = sync_authorized_keys(...).await {
        warn!("failed to sync authorized keys: {e}");   // the member keeps access
    }
    Ok(key)                                              // the caller thinks it's done
}

// Good: the whole removal completes, or the caller retries it
pub async fn remove_member(...) -> Result<Key> {
    let key = revoke_and_rotate(...).await?;
    sync_authorized_keys(...).await?;   // its failure fails the removal
    Ok(key)
}
```

### Periodic cleanup

Deleting what nothing references, on a schedule, is not self-heal: bytes no row
names, a copy a merge replaced, a build no checkout uses. No state is wrong,
since every reference still points at something real; only space is unreclaimed.
Ask what a reader sees before the cleanup runs. A reader who can see wrong state
(a row whose file is missing, access that should be gone) means self-heal, which
is banned; unreclaimed space alone means cleanup, which is allowed.

---
digest: Three kinds of failure, three treatments. Expected failures (network, storage, missing file, unreachable device, bad input) are typed errors shown to the person and logged. Bugs (a broken invariant, a "can't fail" call failing, a use-after-close) are reported to the crash reporter as non-fatal, logged, and shown as "Something went wrong", never silently swallowed. Crash only when continuing could damage data or the process state can no longer be trusted.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
blocking: true
---

**Report bugs without crashing; crash only on state that can't be trusted.**

Every failure is one of three kinds, and each kind has exactly one treatment:

1. **Expected failures** — what happens in normal use: no network, the storage
   provider refuses, a file is missing, a device is unreachable, input is bad.
   These are typed errors carried to whoever can act on them, shown to the
   person where they are looking, and logged. Never a crash, never a report to
   the crash reporter.

2. **Bugs** — the code broke its own guarantee: an invariant failed, a call
   documented as infallible failed, a closed handle was used, a state that
   "can't happen" happened. The process is still sound; one operation is wrong.
   Report it to the crash reporter as a **non-fatal** event (Sentry
   `captureException`, core's anomaly telemetry) with the context needed to fix
   it, log it at error level, and tell the person "Something went wrong" where
   the action happened. The app keeps running. Never swallow it into a no-op: a
   button that silently stops working is the worst outcome, because nobody
   learns it broke.

3. **Untrustworthy state** — continuing could corrupt the library, a sync store,
   or a key; memory or process state is no longer known to be valid; the next
   step would act on garbage. Crash (panic, `fatalError`, uncaught throw), so
   the crash reporter records it and no further damage is done. This is the only
   case that crashes.

Nothing is ignored. "Log only" is reserved for failures with no effect on the
person (best-effort cleanup whose failure changes nothing they see or keep).

The tell for case 2 done wrong: a catch-all wrapper around calls that "can't
fail", logging and continuing. Either the call really can fail (then it's case 1
and needs a typed error the UI shows), or it can't (then a failure is case 2 and
must be reported, not just logged). Prefer closing the hole that lets the bug
happen — e.g. tear screens down before the handle they call closes — over
catching its symptom.

```kotlin
// Bad — a bug becomes a tap that does nothing; nobody is alerted
try { appHandle.playRelease(id) } catch (e: Exception) { logger.error("play failed", e) }

// Good — a bug is reported non-fatally and the person is told
try {
    appHandle.playRelease(id)
} catch (e: Exception) {
    reportBug("playRelease", e)          // Sentry non-fatal + error log
    snackbar.show(somethingWentWrong)    // the person sees it failed
}
```

Platform scopes that run work on their own (coroutine scopes, Swift `Task`s,
spawned tokio tasks) get one handler at the scope that applies case 2, so each
call site doesn't need its own wrapper. UI rendering has no equivalent catch
point on Compose or SwiftUI; a failure there is a crash and is fixed at its
source.

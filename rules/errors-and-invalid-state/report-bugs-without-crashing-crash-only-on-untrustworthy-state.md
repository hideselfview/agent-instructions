---
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
blocking: true
---

## Report bugs without crashing; crash only on state that can't be trusted

Every failure is one of three kinds, and each kind has one treatment:

1. **Expected failures** happen in normal use: no network, the storage provider
   refuses, a file is missing, a device is unreachable, input is bad.
   - Carry a typed error to whoever can act on it, show it to the app's user
     where they are looking, and log it.
   - Never crash, and never report it to the crash reporter.
2. **Bugs** break the code's own guarantee: an invariant failed, a call
   documented as infallible failed, a closed handle was used, a state that
   "can't happen" happened. The process is still sound; one operation is wrong.
   - Report it to the crash reporter as a non-fatal event (Sentry
     `captureException`, the core's anomaly telemetry), with the context needed
     to fix it.
   - Log it at error level, and tell the app's user "Something went wrong" where
     the action happened. The app keeps running.
   - Never swallow it into a no-op: a button that silently stops working is the
     worst outcome, because nobody learns it broke.
3. **Untrustworthy state**: continuing could corrupt the library, a sync store
   or a key, memory or process state is no longer known to be valid, or the next
   step would act on garbage.
   - Crash (panic, `fatalError`, an uncaught throw), so the crash reporter
     records it and no further damage is done.
   - This is the only kind that crashes.

Nothing is ignored. Log a failure and do nothing else only when it has no effect
on the app's user, such as best-effort cleanup whose failure changes nothing
they see or keep.

A bug handled wrong looks like a catch-all wrapper around calls that "can't
fail", which logs and continues. Either the call can fail, and it is an expected
failure that needs a typed error the UI shows, or it can't, and a failure is a
bug that must be reported, not only logged. Prefer closing the hole that lets
the bug happen (tear screens down before the handle they call closes) over
catching its symptom.

```kotlin
// Bad: a bug becomes a tap that does nothing, and nobody is alerted
try { appHandle.playRelease(id) } catch (e: Exception) { logger.error("play failed", e) }

// Good: the bug is reported as non-fatal, and the app's user is told
try {
    appHandle.playRelease(id)
} catch (e: Exception) {
    reportBug("playRelease", e)          // Sentry non-fatal + error log
    snackbar.show(somethingWentWrong)    // the user sees it failed
}
```

Scopes that run work on their own (coroutine scopes, Swift `Task`s, spawned
tokio tasks) get one handler, at the scope, that treats a failure as a bug, so
each call site doesn't need its own wrapper. UI rendering has no such place to
catch a failure in Compose or SwiftUI: a failure there is a crash, fixed at its
source.

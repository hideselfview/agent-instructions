---
digest: Initialize dependencies at the top and pass them down; no singletons; static ambient calls (clock, env, rand) are broken DI — read once at the top or inject a service. But inject existing primitives or what a consumer/test needs — don't invent abstractions with no consumer, and a static call on an already-injected value isn't a violation.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Dependency injection.** Initialize dependencies at the top and pass them down.
No singletons.

Static/ambient calls to system services — the clock (`Utc::now()`), env,
randomness, the filesystem — are dependencies too: read the value once at the
top and pass it down (the pure form injects a service, e.g. a clock /
`TimeService`). Scattering or repeating the static call is broken DI —
especially per-iteration, where each row gets a different `now`. A single read
at the top is fine; the violation is the call buried in a loop or sprinkled
through the body.

**Inject what already exists, or what a consumer needs — not abstractions with
no consumer.** This rule pulls against YAGNI and
`a-mechanism-must-be-load-bearing`; hold both. Inject when (a) a primitive for
it already exists in the codebase — compose the `IdProvider` you already use
elsewhere instead of an ad-hoc `uuid::new_v4()` — or (b) a real consumer needs
the seam now, e.g. a test that injects a fake clock / dispatcher / RNG. Do *not*
invent a new abstraction — a `FileStore` trait, a `Clock` service — for an
ambient call that no such consumer touches; that is dead indirection every
reader carries (the YAGNI failure mode of over-applying this rule).

A static/system call is also **not** a violation when the thing it acts on is
already injected: a storage component doing `std::fs` against a `LibraryDir`
passed into its constructor has its dependency injected (the directory); the
syscalls are its implementation, not hidden ambient state. Likewise genuine
platform mechanisms a unit test would never fake (the camera, the OS keychain).
The target is an *uninjected* ambient dependency — a singleton/global, or a
service call sprinkled through the body — not every appearance of
`fs`/`now()`/`uuid` at a call site.

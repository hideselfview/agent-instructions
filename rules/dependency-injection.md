---
digest: Initialize dependencies at the top and pass them down; no singletons; static ambient calls (clock, env, rand) are broken DI — read once at the top or inject a service.
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

---
digest: Create dependencies at the top and pass them down; no singletons. Calls to the clock, environment, randomness or file system are dependencies too - read once at the top or inject a service. Inject what already exists or what a test needs; don't invent an abstraction nothing uses, and a system call on a value that was already injected is fine.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
blocking: false
---

## Pass dependencies down from the top

Create dependencies at the top of the program and pass them down. No singletons.

Static calls to system services (the clock with `Utc::now()`, the environment,
randomness, the file system) are dependencies too. Read the value once at the
top and pass it down, or inject a service that provides it. A call buried in a
loop or repeated through a body breaks this rule: in a loop, each row gets a
different `now`.

Inject what already exists or what a consumer needs now, and nothing else:

- Use an injected primitive the codebase already has, such as its ID generator,
  instead of a direct `uuid::new_v4()`.
- Add a new injection point when a test needs it now, such as a fake clock,
  dispatcher or random number generator.
- Don't invent a `FileStore` trait or a `Clock` service for a call no test or
  other consumer replaces (`write-only-what-is-needed-today`).

A system call is not a violation when the thing it acts on was injected. A
storage component that calls `std::fs` on a directory passed to its constructor
has its dependency injected: the directory. The same holds for platform services
a unit test would never fake, such as the camera or the OS keychain. The target
is a dependency nobody injected: a singleton, a global, or a service call
scattered through a body.

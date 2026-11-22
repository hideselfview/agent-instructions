---
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

## Check a fact once, where it can become false

Every check answers one question: what goes wrong, and at which step? Put the
check at that step, once, and let every later step trust it. Before you add a
check, name the step where the fact can become false. If that isn't the step you
are writing, the check belongs there, or already exists there: find it.

Banned:

- Checking again on read what a write or transfer already checked: hashing a
  file on every open, checking a cached copy that was checked when it was
  filled, authenticating bytes already authenticated.
- Reading back what was just written to confirm the write. The write's result is
  the answer; a failed write returns an error.
- The same check at several layers. When the caller checked it, the callee takes
  a type that says so, or the caller is its only entry point.
- Checking a fact fixed by construction, or a value just read in the same
  transaction.

A read behaves like the platform's own API. A disk doesn't hash a file when you
open it, and a database doesn't check a row you just committed. A library that
wraps a resource adds checks only where its own promise differs from the
resource's, and says which promise each check protects.

Example: a store hashed a whole file on every open, so playback read the entire
file first, though the upload, the one step where the bytes matter, checks each
chunk it sends.

Checks stay where a fact enters from outside the code's control: bytes from
storage or the network, a file the user can change, input from another process.
These too are checked once, at the point of entry.

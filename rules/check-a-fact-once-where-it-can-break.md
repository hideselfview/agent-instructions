---
digest: Check a fact once, at the step where it can become false, and nowhere else. No re-hashing or re-reading to confirm what an earlier or later step already confirms, no reading back what was just written, no repeating a caller's check in the callee. A read behaves like the platform's own API — opening a file doesn't hash it.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Check a fact once, where it can break.** Every check answers one question:
*what goes wrong, and at which step does it go wrong?* Put the check at that
step, once. Everywhere downstream trusts it.

Banned:

- **Re-verifying on read what a write or transfer already verified.** Re-hashing
  a file on every open, re-checking a cache copy that was checked when it was
  filled, re-authenticating bytes already authenticated.
- **Reading back what was just written** to confirm the write. The write's own
  result is the answer; a failed write returns an error.
- **The same check at several layers.** If the caller validated it, the callee
  takes a type that says so (or the caller is the only entry), not a second
  check.
- **Re-validating a fact fixed by construction**, or a value just read in the
  same transaction.

**A read behaves like the platform's own API.** A disk doesn't re-hash a file
when you open it; a database doesn't re-check a row you just committed. A
library wrapping a resource adds checks only where its own promise differs from
the resource's, and says which promise each check protects.

Worked example: coven hashed a user's whole original on every open of a file
still waiting to upload, so playing a song mid-upload read the entire file
before the first sample. The hash matters only at upload, where changed bytes
would be stored under the row's hash and reach every device, and the upload
already checks each 64 KiB chunk before sending it. The open became what a file
open is: the file exists and its size and modification time match what was
recorded; reads return what's on disk.

Checks stay where a fact enters from outside the code's control — bytes from
storage or the network, a file the user can change, input from another process —
and nothing else checks it. Still once.

Before adding a check, name the step where the fact can become false. If that
step isn't the one you're writing, the check belongs there or already exists
there — find it.

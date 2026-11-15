---
digest: When state the code already holds (a field, an enum tag, a column) decides which source, branch or case applies, dispatch on it; never a fall-through that tries every possibility and takes the first hit. Probe only a fact no state holds, and when you can't find the key that decides, thread it through, model it in the type, or ask, rather than trying every case.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: true
---

## Dispatch on known state; don't try every case

When state the code trusts already decides which source, branch or case applies
(a field on the value, an enum tag, a database column, the gate you just read),
dispatch on that state and go straight to the answer. Never write a fall-through
that tries each possibility in turn and takes the first that works:
`if let Some = tryA() else tryB() else tryC()`, a series of existence checks,
"check every location", a chain of fallbacks.

A fall-through models cases the input rules out: probing source A for a value
whose tag says it can only be in B is work on a branch that can never hit. It
also hides a contradiction: when the state says B but a stale copy is in A,
`tryA() else tryB()` serves the stale copy and reports success, where the
mismatch should have failed loudly
(`no-self-heal-make-state-correct-or-fail-loud`). And it hides the structure
("this value's origin decides its source") behind "we don't know, so we look
everywhere".

```rust
// Bad: probe every source and take the first hit, though the BlobRef carries
// its provenance and the database gate says Local or Remote.
async fn read_blob(blob: &BlobRef) -> Result<Vec<u8>> {
    if let Some(b) = read_external(blob).await? { return Ok(b); }   // only UserProvided is external
    if let Some(b) = read_local_store(blob).await? { return Ok(b); }// only HostProvided+Local is here
    if let Some(b) = read_cache(blob).await? { return Ok(b); }      // only Remote is cached
    read_cloud(blob).await
}

// Good: dispatch on what the value and the gate already say.
async fn read_blob(blob: &BlobRef, locality: Locality) -> Result<Vec<u8>> {
    match (blob.provenance, locality) {
        (UserProvided, _)      => read_external(blob).await,         // the user's own file
        (HostProvided, Local)  => read_local_store(blob).await,      // the only copy
        (HostProvided, Remote) => read_cached_or_cloud(blob).await,  // cache, else fetch
    }
}
```

Probe only a fact that no state holds, one you can learn only by looking. Above,
whether a Remote blob is in `pinned/`, in `cache/` or not fetched yet is a fact
of this device's file system that the shared gate doesn't record, so
`read_cached_or_cloud` checks exactly that. Keep the probe to that one unknown;
never fold the known facts (provenance, locality) back into it.

When you can't find the key to dispatch on, find out why; don't try every case.
State wasn't passed through, a type doesn't model the distinction
(`many-fields-none-together-means-a-missing-type`), or the key is decided
upstream (`fix-a-bug-at-its-cause`). Pass it down, add it to the type, or move
the decision to where the state is. When a path you expect to be deterministic
isn't, and trying every case is the only way forward you can see, stop and ask
the user rather than shipping the search.

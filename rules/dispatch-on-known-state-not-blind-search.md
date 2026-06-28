---
digest: When authoritative state (a field/tag/column on the input) already determines which source/branch/case applies, dispatch on it directly — never a fall-through that probes every possibility and takes the first hit. That models cases the input rules out, wastes work, and silently tolerates state drift. Narrow a genuine runtime check to the one dimension no state holds; if a path you expect to be deterministic isn't, stop and surface it (thread the state through / enrich the type / ask) rather than blind-searching.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: true
---

**Dispatch on known state; don't blind-search for it.** When authoritative state
already determines which source, branch, or case applies — a field on the value,
an enum tag, a DB column, the gate you just read — dispatch on that state and go
straight to the answer. Never write a fall-through that tries every possibility
in turn and takes the first that works.

The anti-pattern is the cascade: `if let Some = tryA() else tryB() else tryC()`,
a sequence of existence probes, a "check every location," a fallback chain —
when the input already carries the discriminator that says which one. It is
wrong on three axes:

- **It models cases the input rules out.** Probing source A for a value whose
  tag says it can only be in B is dead work over a branch that can never hit.
  The impossible case in the search is the tell that the discriminator wasn't
  used.
- **It silently tolerates drift** — the same sin as
  `no-self-heal-make-state-correct-or-fail-loud`. If the authoritative state
  says "B" but a stale artifact sits in A, a blind `tryA() else tryB()` serves
  the stale A and *reports success*. You never learn the state is corrupt; the
  search papered over a contradiction that should have failed loud.
- **It obscures the real shape.** The structure is "this value's provenance /
  locality / kind determines its source." A search hides that behind "we don't
  know, so we look everywhere," and every reader inherits the false belief that
  any source could answer.

The robustness it *feels* like it buys — "handles every case without committing
to knowing which" — is the hedge. Knowing which case it is, is the design;
searching is the refusal to.

```rust
// Bad — blind fall-through: probe every source, take the first hit. The BlobRef
// already carries `provenance`; the DB gate already says Local vs Remote.
async fn read_blob(blob: &BlobRef) -> Result<Vec<u8>> {
    if let Some(b) = read_external(blob).await? { return Ok(b); }   // only UserProvided is ever external
    if let Some(b) = read_local_store(blob).await? { return Ok(b); }// only HostProvided+Local lives here
    if let Some(b) = read_cache(blob).await? { return Ok(b); }      // only Remote is ever cached
    read_cloud(blob).await
}
// Probes external for a HostProvided blob and the local store for a Remote blob —
// both impossible. And if the gate says Remote while a stale local-store file
// lingers, this serves it and never surfaces the corruption.

// Good — dispatch on what the value and the authoritative state already say.
async fn read_blob(blob: &BlobRef, locality: Locality) -> Result<Vec<u8>> {
    match (blob.provenance, locality) {
        (UserProvided, _)      => read_external(blob).await,         // the user's own file
        (HostProvided, Local)  => read_local_store(blob).await,      // its home, the only copy
        (HostProvided, Remote) => read_cached_or_cloud(blob).await,  // cache, else fetch
    }
}
```

**The one legitimate probe** is a dimension *no* authoritative state holds — a
genuine runtime fact you can only learn by looking. Above, "is this Remote blob
in `pinned/` vs `cache/` vs not-yet-fetched" is a per-device filesystem fact the
global gate doesn't capture, so `read_cached_or_cloud` checks exactly that, and
*only* that. Keep the check narrow to the unknown dimension; never fold the
known ones (provenance, locality) back into the search.

**When you can't find the dispatch key**, that is the signal — not the cue to
blind-search. The key is missing where you need it because state wasn't threaded
through, a type didn't model the distinction (see
`many-fields-none-together-means-a-missing-type`), or the discriminator lives
upstream (`fix-at-the-lowest-level-the-bug-allows`). Thread it down, enrich the
type, or move the decision to where the state lives. If a pathway you expect to
be deterministic genuinely isn't — and a blind search is the only way you can
see to proceed — **stop and surface it to the user** rather than shipping the
search. "I don't know which case this is" is answered by *finding out which case
it is*, not by trying them all.

*(Pairs with `no-self-heal-make-state-correct-or-fail-loud` — the search
tolerates the drift that rule forbids; with
`many-fields-none-together-means-a-missing-type` and
`state-describes-what-is-not-what-should-happen` — the missing discriminator is
often a type that wants modeling; and with the revealing-structure principle —
dispatch reveals the shape a search hides.)*

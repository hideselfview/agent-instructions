---
digest: Don't re-implement the unit under test/preview; faking its dependencies is fine.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Never re-implement the unit under test or under preview.** A test exercises
its target's real production code; a `#Preview` of `Foo` renders `Foo`'s real
`body`. The rule is scoped to the *unit being exercised* — the thing the test or
preview is supposed to validate. Its *dependencies* are out of scope: those can
be stubbed, faked, or injected with hand-rolled values to give the unit
something to run against.

If the unit itself is duplicated, the test or preview is checking the duplicate
against itself, not validating what ships.

Anti-patterns (re-implementing the unit):

- A `FooTestImpl` that hand-codes the same business logic `Foo` has — and the
  test exercises `FooTestImpl` instead of `Foo`.
- A test helper that constructs the expected output by running its own version
  of the algorithm under test.
- Mocking so deep that the test exercises the mock chain, not the production
  module.
- A SwiftUI `#Preview` of `Foo` that rewrites `Foo.body` (e.g. a bespoke
  preview-only view that mimics `Foo`'s layout) — the preview no longer shows
  what ships.

Fine and often necessary (faking dependencies):

- Passing a fake HTTP client, in-memory database, frozen clock, deterministic
  RNG, or other stub *dependency* to the unit. Production passes the real one;
  the test passes a fake. The unit's own logic is unchanged.
- A `#Preview` that injects stub `@Environment` values, fake stores, or
  hand-built fixture data so the real view has something to render against. Only
  `Foo.body` runs the production path; what it reads through the environment can
  be synthesized.

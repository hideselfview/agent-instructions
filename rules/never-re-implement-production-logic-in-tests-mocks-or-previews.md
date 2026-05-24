---
digest: Tests exercise real production code with mock inputs; don't re-derive its outputs.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Never re-implement production logic in tests, mocks, or previews.** Tests
should exercise the real production code with mock *inputs*, not re-derive its
outputs. If your test setup duplicates what production does, you're checking the
duplicate against itself, not validating production. Anti-patterns:

- A `FooTestImpl` that hand-codes the same business logic `Foo` has.
- A test helper that constructs the expected output by running its own version
  of the algorithm.
- Mocking so deep that the test exercises the mock chain, not the production
  module.
- A SwiftUI `#Preview` that rewrites the view's body to make it render — now the
  preview shows a parallel implementation, not what ships.

---
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
blocking: false
---

## Never re-implement the unit under test or preview

A test runs its target's production code, and a picture of a view (a screenshot
test or an IDE preview) draws the real view. When the unit itself is copied, the
test checks the copy against itself, not what ships. The rule covers only the
unit being checked: its dependencies may be stubbed, faked or filled with values
made by hand, so the unit has something to run against.

Re-implementing the unit, banned:

- A `FooTestImpl` that copies `Foo`'s logic, which the test runs instead of
  `Foo`.
- A test helper that computes the expected output with its own version of the
  algorithm under test.
- A preview of `Foo` that draws a look-alike view instead of `Foo`.

Faking dependencies, allowed:

- Passing the unit a fake HTTP client, an in-memory database, a frozen clock or
  a seeded random number generator. Production passes the real one, and the
  unit's own logic is unchanged.
- Giving the real view stub environment values, fake stores or fixture data to
  draw.

---
digest: A bug fix starts with a test that fails on the real code, fixes the bad value where it comes from rather than in the consumer that trips on it, and commits only the change for the confirmed cause; investigation logs and mechanisms built for a disproved cause come out.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.py'
blocking: false
---

## Fix a bug at its cause

A bug fix starts with a test that fails, changes the code where the bad value
comes from, and commits that change and nothing else.

### Start with a failing test

Before you debug, write a test that reproduces the failure, and run it to see it
fail. Then fix the code, and run the test to see it pass. Every fix starts this
way, however obvious: the failing test shows you understood the bug, and the
passing test shows the fix addressed it. When you describe a fix, give the test
("The test: …") before the fix.

The test calls the real function or service that has the bug. A test that
rebuilds the conditions by hand, calling sub-functions in the order you think
causes the bug, tests another program
(`never-re-implement-production-logic-in-tests-mocks-or-previews`).

### Fix where the bad value comes from

Trace the bad value back through each step until you reach a layer you don't
control (the network, the file system, the user), and fix it in the first layer
you do control. A fix in a consumer protects one call site; a fix at the source
protects every reader, now and later. A fix that is a `filter(...)`,
`?? default`, `if let Some(x), !x.is_empty()`, `try?` or `guard let`, there only
to clean a bad value from a producer, means the producer has the wrong shape.

Example: Discogs returns `"thumb": ""` for a release with no cover, and serde
reads it as `Some("")`. A filter in the search-result mapper
(`r.thumb.clone().filter(|s| !s.is_empty())`) fixes one reader, and every other
reader of the field still sees `Some("")`. The fix goes on the field, so the
field is either `None` or a URL:

```rust
// A shared serde helper
pub(crate) fn empty_string_as_none<'de, D>(
    d: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(d)?.filter(|s| !s.is_empty()))
}

// On the field: written once, it covers every reader
#[serde(default, deserialize_with = "crate::serde_helpers::empty_string_as_none")]
pub thumb: Option<String>,
```

The source of other bugs:

- A bad value in an API's JSON: a deserializer on the field, or
  `serde(with = …)` on the type.
- A field the type requires that can be missing: make it `Option<T>` and update
  the callers, rather than `.unwrap_or_default()` in a consumer.
- A function that receives the wrong shape: fix the caller, or change the
  signature so the wrong shape can't be passed, rather than a guard in the body.
- An event that lacks a field: add the field to the payload at the emitter,
  rather than looking it up from other state in the reducer
  (`reducers-must-not-read-state-to-write-state`).

The consumer is the right place only when its context is what makes the value
bad. An empty `description` is fine to store, but shouldn't show as a tooltip:
the renderer decides that.

### Commit the fix, not the investigation

A fix's diff holds the change for the confirmed cause and nothing else. Before
you commit, remove:

- Code added to observe: logs, prints, `dbg!`, counters, timing, scratch
  asserts, `if id == "the-one" { … }`.
  - A log worth keeping goes through the project's logger at the right level
    (`use-the-project-s-logger-for-real-logs`).
- A mechanism, type, guard, cache or flag built for a cause the evidence later
  ruled out, or never confirmed.
  - This code compiles and has tests, so it is the residue most often left in.

For each hunk, name the evidence that makes it necessary. "It's still correct,
and harmless to keep" is not enough: a correct mechanism for a problem that
doesn't exist is still code every reader must carry. A real improvement of its
own goes in its own commit, with its own reason.

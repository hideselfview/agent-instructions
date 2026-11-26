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

## Write only what is needed today

Before any fix, refactor or addition, ask whether it is needed, and what could
change so that it isn't. The duplicate may already be handled upstream, the bug
may be a symptom of a deeper one, and a new abstraction may disappear once the
right concept exists. Ask before you write the code, and again for code that is
already there.

### Today's shape

Model the logic as it is today, never a shape meant to make a later change
smaller. When today's logic returns zero or one item, the type is `Option<T>`,
not `Vec<T>` because a later change might return two. Code is cheap to rewrite:
the later change widens `Option` to `Vec` when it arrives, while a shape that is
wrong today burdens every reader, test and review until then.

### Dead code

Code that nothing outside tests calls is dead. Whoever finds dead code deletes
it, and never asks the person whether to keep it:

- When the dead code is in the work you are doing, delete it in that work.
- When it is outside that work, add an item to the work queue at once that
  deletes it.

A commit must not add code that nothing outside tests calls: the code lands with
its caller, or not at all.

When dead code on main is the only code that does a behavior the spec states,
ask the person whether the behavior stays, and quote the spec's text. When it
stays, build the code's caller; when it goes, delete the code and the spec's
text.

Delete dead code at its root, not around it: a filter or guard in the consumer
keeps the dead code alive. After the deletion, fields that existed only for the
dead case become required.

- When a function loses its last caller, its callees that only it called are
  dead too: delete the whole chain.
  - Rust's `dead_code` lint finds such chains only for non-`pub` items; trace a
    `pub` chain across crates by hand.
- Grep for every caller of a definition, not only the path you came in by.
- When the only callers are tests, delete the tests too.

Examples:

- A `Complete` event variant is emitted, and no consumer uses it.
  - Delete the emission, the variant, and the optional fields that existed
    because it left them empty. Don't filter it in the consumer.
- An optional field is always `None` at one call site.
  - Find out why, rather than adding a check at the call site.
  - When no call site sets it, delete the field.
  - When only some do, two concepts share one type: split them.
- An enum case nothing creates: remove it from the enum, rather than skipping it
  in a match arm.
- A parameter that never varies, or a field nothing reads: delete it.

### Unused code in Rust

When the compiler warns about unused code, delete it, or restrict when it
compiles with `#[cfg(...)]`:

- `#[cfg(test)]` for test-only code.
- `#[cfg(feature = "…")]` for code behind a feature.
- `#[cfg(target_os = "macos")]` for code for one platform.

Never `#[allow(dead_code)]`: it hides the decision for good, while `#[cfg]`
states the context and fails to compile when production code uses test-only
code.

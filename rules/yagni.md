---
digest: No dead code; delete at the root, not around it; dead branches, never-varying params and unread fields go; fields that modeled the dead case become required; in Rust, delete unused code or #[cfg]-restrict it, never #[allow(dead_code)].
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**YAGNI.** Don't leave dead code around. Delete at the root, not at the consumer
— filtering or guarding around dead code preserves it. After deletion, fields
that existed only to model the dead case become required.

Dead code cascades. Deleting a consumer can orphan a whole call chain: when a
function loses its last caller, check its callees — any whose only caller was
that function are dead too. Trace to the deepest dead node and delete the entire
chain, not just the top. (Rust's `dead_code` lint does this transitively, but
only for non-`pub` items; a `pub` chain across a crate boundary stays silent, so
trace it by hand.) *(See `principles/yagni.md`. Pairs with question-necessity:
YAGNI is retrospective, question-necessity is prospective.)*

### Rust: delete unused code or `#[cfg]`-restrict it, never `#[allow(dead_code)]`

When the compiler warns about unused code, two options:

1. **Delete it.** Preferred. We don't keep unused code around.
2. **Restrict when it exists** with `#[cfg(...)]`:
   - Test-only: `#[cfg(test)]`
   - Feature-gated: `#[cfg(feature = "some-feature")]`
   - Platform-specific: `#[cfg(target_os = "macos")]`

`#[allow(dead_code)]` is banned: it hides the decision behind a permanent
suppression. `#[cfg(...)]` makes the context explicit and forces a compiler
error if test-only code is used in production.

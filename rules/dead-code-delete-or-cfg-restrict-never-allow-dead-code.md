---
digest: Delete unused code or #[cfg]-restrict it; never #[allow(dead_code)].
paths:
  - '**/*.rs'
blocking: false
---

## Dead code: delete or `#[cfg]`-restrict, never `#[allow(dead_code)]`

When the compiler warns about unused code, two options:

1. **Delete it.** Preferred. We don't keep unused code around.
2. **Restrict when it exists** with `#[cfg(...)]`:
   - Test-only: `#[cfg(test)]`
   - Feature-gated: `#[cfg(feature = "some-feature")]`
   - Platform-specific: `#[cfg(target_os = "macos")]`

`#[allow(dead_code)]` is banned — it hides the decision behind a permanent
suppression. `#[cfg(...)]` instead makes the context explicit and forces a
compiler error if test-only code is used in production.

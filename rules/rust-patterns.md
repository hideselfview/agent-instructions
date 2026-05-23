---
paths:
  - '**/*.rs'
---

# Rust patterns

## Enums

- Don't derive `Default` on enums or use `#[default]` attributes.
- Put associated data directly in variants, not in separate fields.

```rust
// Bad
enum Mode { Created, Loading, Ready }
struct State { mode: Mode, loading_id: Option<String> }

// Good
enum Mode { Created, Loading(String), Ready }
```

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

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

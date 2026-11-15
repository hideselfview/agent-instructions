---
digest: Several fields set to None together in one branch mean a missing type; extract a sub-struct or an enum variant, and put a variant's data in the variant. In Rust, no `Default` or `#[default]` on an enum.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: true
---

## Many fields set to None together mean a missing type

A per-field `Option` means that field is optional on its own in the domain. When
code sets several fields to `None` or `nil` in one branch, those fields are
absent together, and the type is missing the variant that says so. Every reader
then has to know the hidden condition. Move the fields that change together into
their own type: a sub-struct, or a variant of an enum on the parent.

```rust
// Bad: the block that clears every pressing field shows a missing type
let mut edit = ReleaseUserEdit {
    year: detail.year,
    format: detail.format,
    label: detail.label,
    country: detail.country,
    barcode: detail.barcode,
    ..
};
if matches!(choice, Approximate | Unknown) {
    edit.year = None;
    edit.format = None;
    edit.label = None;
    edit.country = None;
    edit.barcode = None;
}

// Good: name the group
struct PressingEdit { year, format, label, country, barcode, .. }
impl PressingEdit { fn blank() -> Self { ... } }

let pressing = match choice {
    Exact { .. } => PressingEdit { year: detail.year, ... },
    Approximate { .. } | Unknown => PressingEdit::blank(),
};
let edit = ReleaseUserEdit { album_title, pressing, tracks };
```

The same holds for an enum with a field beside it that only one variant uses:
put the data in the variant.

```rust
// Bad
enum Mode { Created, Loading, Ready }
struct State { mode: Mode, loading_id: Option<String> }

// Good
enum Mode { Created, Loading(String), Ready }
```

In Rust, don't derive `Default` on an enum or mark a variant `#[default]`.

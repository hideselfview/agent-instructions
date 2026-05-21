**Many fields → `None` together means a missing type.**

> Blocking

When you see code that sets several fields of a struct to `None`/`nil` in the
same conditional block, the type is hiding a missing variant. The fields that
flip together belong in their own type — extract a sub-struct, or model the
distinction as an enum variant on a parent type. Per-field `Option` should mean
"this field is individually optional in the domain." Using per-field `Option`s
to express "this whole subset is absent" overloads the type and forces every
consumer to know the implicit discriminator.

```rust
// Bad — the "every pressing field nilled" block reveals a missing type.
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

// Good — name the cluster.
struct PressingEdit { year, format, label, country, barcode, .. }
impl PressingEdit { fn blank() -> Self { ... } }

let pressing = match choice {
    Exact { .. } => PressingEdit { year: detail.year, ... },
    Approximate { .. } | Unknown => PressingEdit::blank(),
};
let edit = ReleaseUserEdit { album_title, pressing, tracks };
```

Naming the discriminator structurally (sub-struct, enum variant) makes the
absence visible at the type level instead of buried in per-consumer
conditionals.

---
digest: Split code, SQL and tests along domains (what owns a set of data and the rules that keep it right), never along a topic several domains share; a domain's tables, triggers, code and tests stay together. A type, layer or method that exists only to get around a crate or module boundary is the wrong shape; merge it, make the extra method a function in the higher crate, or move the type to where it belongs.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
  - '**/*.sql'
blocking: false
---

## Organize code by domain

A domain owns a set of data and the rules that keep it right. Its data is
written only through it, and its tables, triggers, code and tests are kept
together. Split files, place tests and move schema along domains. Crates,
modules and files serve that structure: nothing in the code exists only because
of where the code is kept.

### Split by domain, not by topic

A topic is a word several domains share ("covers", "progress", "CUE", "SQL"). A
file grouped by topic mixes code from different owners, and the reader can no
longer tell who keeps which data right.

- A domain in bae: the import list.
  - It owns the `import_list_placement` table and the triggers that keep each
    row's place.
  - Its reads and writes are in `bae-import/src/tables/import_list.rs` and its
    `order`, `window` and `writes` modules, with the tests of all of them.
  - Its schema and trigger tests are beside it, not in a shared schema file.
- A topic, not a domain: covers.
  - A release's stored cover, the covers an import candidate offers and the
    images a catalog answer lists belong to three domains with different rules.
  - Each goes with its own domain, not into one `covers.rs`.

Each table has one domain that writes it; other domains read it.

### Remove structure that only gets around a boundary

Crate order, visibility and file layout push code into shapes the domain doesn't
have: a wrapper added so a method can be in a higher crate, two names for one
thing, a layer that only passes a value along. Each makes a reader ask which of
the two is the owner, when there is only one. When a type, layer or method
exists only to get around where code is kept, remove it.

Signs:

- A struct whose only field is another type, and whose methods mostly forward to
  that field's methods of the same name.
- One idea under two names in two crates.
- A wrapper whose only addition is a method that needed a higher crate.
- A capability passed through owners that only forward it and never use it.

Fixes:

- Merge the two into one type.
- Turn the extra method into a function in the higher crate that takes the type.
- Move the type to the crate where it belongs.

```rust
// Bad: StoreLayout wraps StoreDirectories only to add stores(), which needs a
// crate that StoreDirectories' crate can't depend on.
pub struct StoreLayout {
    directories: StoreDirectories,
}
impl StoreLayout {
    pub fn store_dir(&self, id: &StoreId) -> StoreDir {
        self.directories.store_dir(id) // forwards; adds nothing
    }
    pub fn stores(&self) -> Result<Vec<StoreInfo>, ListError> { /* reads names */ }
}

// Good: one type makes StoreDir, and listing stores is a function in the
// higher crate.
pub use coven_foundation::files::StoreDirectories;
pub fn stores(layout: &StoreDirectories) -> Result<Vec<StoreInfo>, ListError> { /* … */ }
```

A wrapper that holds a distinct state with its own invariants is a real type
(`Request` and `AuthorizedRequest`): keep it. Ask whether the domain has two
things, or only the build does.

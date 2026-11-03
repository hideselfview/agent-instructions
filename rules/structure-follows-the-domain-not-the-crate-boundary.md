---
digest: A type, layer or method that exists only to get around a crate or module boundary, and isn't a separate thing in the domain, is the wrong shape; merge it, move the extra method up as a function, or move the type to where it belongs.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Structure follows the domain, not the crate boundary.**

Crate order, visibility and file layout push code into shapes the domain doesn't
have: a wrapper added so a method can live one crate up, two names for one
thing, a layer that only passes a value along. Each one makes a reader ask which
of the two is the real owner, when there is only one. When a type, layer or
method exists only to get around where code lives, remove it.

**Signs:**

- A struct whose only field is another type, and whose methods mostly forward to
  that field's same-named methods.
- One idea under two names in two crates.
- A wrapper whose only real addition is a method that needed a higher crate.
- A capability that passes through owners which only forward it and never use it
  themselves.

**Fixes:**

- Merge the two into one type.
- Turn the extra method into a free function in the higher crate, taking the
  type.
- Move the type to the crate where it belongs.

```rust
// Bad: coven's StoreLayout wraps StoreDirectories only to add stores(),
// which needs coven-database, a crate StoreDirectories' crate sits below.
pub struct StoreLayout {
    directories: StoreDirectories,
}
impl StoreLayout {
    pub fn store_dir(&self, id: &StoreId) -> StoreDir {
        self.directories.store_dir(id) // forwards; makes nothing
    }
    pub fn stores(&self) -> Result<Vec<StoreInfo>, ListError> { /* reads names */ }
}

// Good: one type makes StoreDir; listing by name is a function one crate up.
pub use coven_foundation::files::StoreDirectories;
pub fn stores(layout: &StoreDirectories) -> Result<Vec<StoreInfo>, ListError> { /* … */ }
```

A wrapper that holds a distinct state with its own invariants is a real type
(`Request → AuthorizedRequest`); keep it. The test is whether the domain has two
things or the build does.

Related: `principles/revealing-structure.md` ("Wrapper chain"),
`rules/don-t-create-duplicate-types.md`.

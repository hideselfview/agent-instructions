---
digest: A handle lets its holder control, watch or guard something its owner runs, across a boundary, and nothing more. It holds only state its owner made to share with it, never a capability or an owner, by value, through a shared or weak pointer, or by reference. Work that needs a capability runs in the owner.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
blocking: true
---

## A handle controls, watches or guards

An owner is an object that holds a capability (a database connection, the
network, files, the clock) or holds another owner. A handle is a value an owner
hands out on purpose, so that code across a boundary can act on what the owner
runs. Every handle is one of three kinds:

- **Control:** the holder sends the owner a request, as a live query's request
  sender replaces the query's request.
- **Watch:** the holder receives what the owner reports, as a progress watch
  receives each step of an open.
- **Guard:** the holder asks the owner for something while the guard lives, and
  dropping the guard ends the request, as a sync guard asks for more frequent
  sync.

A handle holds only state its owner made to share with it: a channel's end, a
watch, a counter, or a lock the owner keeps. A handle never holds a capability
or an owner, by value, through `Arc`, `Rc` or `Weak` (or a Swift or Kotlin
reference to one), or by reference, by itself or inside a type it holds. Work
that needs a capability runs in the owner, and the holder asks the owner for it
through the owners above.

A handle is never a way to reach an owner's state from outside. A value that
does the owner's work itself, such as a query that carries the database's
readers and runs SQL on the caller's thread, is not one of the three kinds, so
it is not a handle, whatever its name.

Flag each handle type (one a project's checker policy lists as a handle, or one
an owner returns for a holder across a boundary) that:

- holds a capability or an owner, directly or inside a type it holds;
- holds an owner's whole inner state when it uses only part of it, as a marker
  that holds the whole files owner to clear one flag;
- does work that needs a capability, instead of asking its owner.

Example: a connection marker holds `Arc<FilesInner>` and, when dropped, clears
the storage slot's `attempting` flag. Hold the storage slot's sender instead:
that sender is the state the owner made to share, and the files owner's database
and directory stay with the owner.

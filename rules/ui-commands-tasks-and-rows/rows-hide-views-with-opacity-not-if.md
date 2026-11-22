---
paths:
  - '**/*.swift'
blocking: false
---

## Rows hide views with opacity, not if

Inside `ForEach` and `List` rows, or any repeated container, keep every child
that hover, focus or selection shows or hides in the view tree at all times.
Show and hide such a child with `.opacity(0/1)` and
`.allowsHitTesting(false/true)`, never with `if condition { SomeView() }`.
Adding or removing a child changes the row's size, which invalidates the parent
stack and makes SwiftUI measure every sibling row again: a hover that adds a
button to each row of a long list drops the frame rate below one frame a second.

Outside repeated views, such as a sheet or a single detail view, an `if` is
fine.

A row may leave out with `if` a line that only the row's data sets and that no
hover, focus or selection changes, such as a status line that exists only for
some records. The row's size then changes only when its data changes, and
SwiftUI measures the row again at that point anyway. Hover, focus and selection
still show and hide their children with opacity.

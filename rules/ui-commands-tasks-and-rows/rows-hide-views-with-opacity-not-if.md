---
digest: In SwiftUI ForEach and List rows, keep every child in the view tree and show or hide it with opacity and hit testing, never with an if.
paths:
  - '**/*.swift'
blocking: false
---

## Rows hide views with opacity, not if

Inside `ForEach` and `List` rows, or any repeated container, keep every child
the row can show in the view tree at all times. Show and hide a child with
`.opacity(0/1)` and `.allowsHitTesting(false/true)`, never with
`if condition { SomeView() }`. Adding or removing a child changes the row's
size, which invalidates the parent stack and makes SwiftUI measure every sibling
row again: a hover that adds a button to each row of a long list drops the frame
rate below one frame a second.

Outside repeated views, such as a sheet or a single detail view, an `if` is
fine.

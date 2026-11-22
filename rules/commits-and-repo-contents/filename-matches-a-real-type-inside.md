---
paths:
  - '**/*.swift'
blocking: false
---

## Filename matches a real type inside

Every `.swift` file's name names something the file contains. A name that
matches nothing inside leaves the reader looking for the type, and the file
collects unrelated code over time. Three shapes are allowed:

1. **One primary type.** `LightboxView.swift` contains `struct LightboxView`.
   - Types tied to the primary one, such as the data it takes or its sub-views,
     may share the file.
2. **An extension of one type.** `BridgeArtistSortField+CaseIterable.swift`
   extends `BridgeArtistSortField`, in Apple's `<Type>+<Aspect>.swift` form.
3. **A caseless enum used as a namespace.** `ImageLoader.swift` contains
   `enum ImageLoader { static func … }`.
   - Use it to group free functions or related types under one name: the public
     API inside the enum, private helpers at file scope.

Tests may group by area: a test file may hold several `*Tests` suites under the
area's name.

---
paths:
  - "**/*.swift"
---

# SwiftUI rules

## Read at the leaf, not the parent

Pass the observable object (store, appService) down. Let the leaf view read
the specific field it needs. Don't read in a parent and pass the value —
that subscribes the parent to changes it doesn't care about. SwiftUI tracks
which view accessed which `@Observable` property; reading in a parent widens
the re-render scope unnecessarily.

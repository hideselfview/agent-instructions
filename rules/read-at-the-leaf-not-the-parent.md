---
digest: Pass the observable down and read the field at the leaf; reading in a parent widens re-render scope.
paths:
  - '**/*.swift'
  - '**/*.tsx'
  - '**/*.jsx'
  - bae-web/**/*.rs
blocking: false
---

## Read at the leaf, not the parent

Pass the observable object (store, app service) down. Let the leaf component
read the specific field it needs. Don't read in a parent and pass the value —
that subscribes the parent to changes it doesn't care about. The reactive
framework tracks which view accessed which property; reading in a parent widens
the re-render scope unnecessarily.

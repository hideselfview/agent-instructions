---
digest: Combine existing primitives before adding a new one; a domain name in a new primitive is a tell.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Compose existing primitives before adding new ones.** When a need arises, list
the primitives already in the system that touch the concern and sketch how they
combine into the answer. Adding a new primitive — a function, method, keyword,
endpoint, trait, config flag — when existing ones combine is a major red flag. A
domain name in the new primitive's name (a specific provider, ontology, vendor,
external service) is a tell that you're baking a specific case into vocabulary
that should stay generic. *(See `principles/composable-primitives.md`. Pairs
with question-necessity and YAGNI.)*

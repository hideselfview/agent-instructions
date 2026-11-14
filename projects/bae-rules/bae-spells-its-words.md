---
digest: bae is always lowercase and Disc ID always has both capitals in user-visible strings (UI, docs, errors, filenames, URLs, the spec); code identifiers exempt.
paths:
  - '**/*'
blocking: true
---

## Stylization: how bae spells its words

Two words have one spelling in every user-visible string: UI text, labels, docs,
the spec, error messages, window titles, button text, alt text, meta
descriptions, filenames and URLs. Code identifiers (variables, functions, types)
follow language conventions in both cases.

**bae is always lowercase.** Examples: `bae.dmg`, `bae Library`, `bae://`. Never
"Bae" or "BAE". Env var names (`BAE_PORT`), HTTP header names
(`X-Bae-Signature`) and other wire or config keys mentioned in prose are
identifiers, not user-visible strings, and keep their own spelling.

**Disc ID is always written with a capital D and capital ID**, including in the
middle of a sentence ("Ask about this Disc ID again"). Never "disc ID", "disc
id", "discid" or "Disc Id". Code identifiers such as `disc_id` and
`DiscIdSignal` follow language conventions.

---
paths:
  - '**/*'
blocking: true
design: true
---

## Spell bae and Disc ID one way

Two words have one spelling in every user-visible string: UI text, labels, docs,
the spec, error messages, window titles, button text, alt text, meta
descriptions, filenames and URLs. Code identifiers (variables, functions, types)
follow their language's conventions for both.

**bae is always lowercase**, as in `bae.dmg`, `bae Library` and `bae://`, never
"Bae" or "BAE". Environment variable names (`BAE_SENTRY_DSN`) and other wire or
config keys mentioned in prose are identifiers, not user-visible strings, and
keep their own spelling.

**Disc ID is always written with a capital D and capital ID**, including in the
middle of a sentence ("Ask about this Disc ID again"), never "disc ID", "disc
id", "discid" or "Disc Id". Code identifiers such as `disc_id` and
`DiscIdSignal` follow their language's conventions.

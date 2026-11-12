---
digest: A bae UI string never breaks a sentence with a dash (em dash, or an en dash with spaces); a pause is a comma, a new thought a new sentence, a definition a colon.
paths:
  - bae-macos/**/*.swift
  - bae-ios/**/*.swift
  - bae-android/**/*.kt
  - bae-android/**/strings.xml
  - bae-macos/**/Localizable.xcstrings
  - bae-bridge/loc/catalog.toml
  - BaeKit/**/*.swift
blocking: true
---

## No dash as sentence punctuation in UI strings

A string the person reads (a label, an empty state, a footer, an error) never
joins or breaks its sentences with a dash: no em dash (—), and no en dash with
spaces around it ( – ), which is the same mark in European typography.

Write what the dash stood for:

- **A pause or an aside:** a comma. "…where your last import went, including
  identifications you start yourself."
- **A second thought:** a new sentence. "No matches. Try different terms."
- **A definition or a list that follows:** a colon.

The English source is held to it mechanically: `scripts/loc-english-skeleton.py`
fails on a dash in a macOS or Android English source, and bae-loc's catalog
check on one in a `core.*` message. Translations follow the same rule as their
source: a translator renders the comma, the full stop or the colon in the
language's own form, not a dash. The one exception is a dash a language's
grammar requires, such as the dash Ukrainian writes for an omitted "is" ("Тека —
це рип із CD"); that dash is the grammar, not an aside.

Not covered: text no person reads in the app. That includes code comments, docs,
test and shot scene names and fixture data.

---
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
joins or breaks its sentences with a dash, and writes what the dash stood for
(`rules/text-people-read/write-plain-literal-active-sentences.md`, No dash as
sentence punctuation).

Two checks hold the English source to it: `scripts/loc-english-skeleton.py`
fails on a dash in a macOS or Android English string, and bae-loc's catalog
check fails on one in a `core.*` message. Translations follow the same rule as
their source: a translator renders the comma, the full stop or the colon in the
language's own form, not a dash. The one exception is a dash a language's
grammar requires, such as the dash Ukrainian writes for an omitted "is" ("Тека —
це рип із CD"); that dash is the grammar, not an aside.

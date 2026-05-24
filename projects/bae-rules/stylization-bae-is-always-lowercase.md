---
digest: bae is always lowercase in user-visible strings (UI, docs, errors, filenames, URLs); code identifiers exempt.
paths:
  - '**/*'
blocking: true
---

## Stylization: bae is always lowercase

"bae" is always lowercase in user-visible strings — UI text, labels, docs, error
messages, window titles, button text, alt text, meta descriptions, filenames
(`bae.dmg`, `bae Library`), URLs (`bae://`). Never "Bae" or "BAE".

Exception: code identifiers (variables, functions, types) follow language
conventions. This covers env var names (`BAE_PORT`), HTTP header names
(`X-Bae-Signature`), and other wire/config keys mentioned in prose — they're
identifiers, not user-visible strings.

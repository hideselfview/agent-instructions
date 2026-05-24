---
digest: Never use real artist/album/song names in durable artifacts; use descriptive placeholders.
paths:
  - '**/*'
blocking: true
---

## No real artist/album/song names in artifacts

Never use real artist, album, or song names in any durable written artifact:
code, tests, UI strings, docs, mockups, PR titles/descriptions, commit messages,
plan docs, issue bodies. Use descriptive placeholders that carry the same
meaning — "2×LP vinyl rip", "the release", "Artist Name", "Album Title", "Track
Title", "rel-123".

The only safe place for a real name is ephemeral chat that won't be indexed or
linked later. Before finalizing any PR title/body, commit message, or plan doc,
scan and replace.

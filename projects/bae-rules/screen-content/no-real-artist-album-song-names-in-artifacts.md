---
digest: Never use real artist, album or song names in durable artifacts; use descriptive placeholders.
paths:
  - '**/*'
blocking: true
---

## No real artist, album or song names in artifacts

Never use real artist, album or song names in any durable written artifact:
code, tests, UI strings, docs, boards, PR titles and descriptions, commit
messages, plans and issue bodies. Use descriptive placeholders that carry the
same meaning, such as "2×LP vinyl rip", "the release", "Artist Name", "Album
Title", "Track Title" and "rel-123".

A real name may appear only in chat that nothing indexes or links to later.
Before you finish a PR title or body, a commit message or a plan, check it for
real names and replace them.

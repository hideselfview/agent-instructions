---
digest: Every surfaced error must have a display path — global alert, inline, or candidate error state.
paths:
  - '**/*.swift'
blocking: false
---

## Error display path

Every surfaced error must have a display path:

- `AppService.lastError` shows via the global alert.
- Per-view errors show inline.
- Import errors show in the candidate's error state.

If you add a new error path, verify it's displayed somewhere before committing.

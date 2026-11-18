---
digest: Before adding UI for an edge case, make it impossible or settle it from the facts; add UI only for what only the app's user knows, and only once the user you work for agrees.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
  - '**/*.md'
blocking: false
---

## Settle edge cases from the facts, not with special-case UI

Don't answer a conflict, an ambiguity or a rare state with a screen, banner,
picker, prompt, dialog, button or extra flow. "Let them pick" hands the problem
to the app's user, and it adds a view on every platform, strings in every
locale, and a state each of them must render. Before you propose any of that,
ask in order:

1. **Can the state be made impossible?** A constraint, a single owner, an order
   of writes.
2. **Do the facts already decide it?** An id names one thing, so two rows
   holding it are one row. A rule every device applies alike, such as keeping
   the smallest id, settles a tie without asking anyone.
3. **Can the operation fail loudly to whoever started it, and be retried?**

UI is the answer only when the outcome depends on something only the app's user
knows: taste, intent, a fact that is not in the data. Even then the user you
work for decides, not you: raise it as a question, with the alternatives that
use only the facts beside it, and build nothing until they choose.

Example: two devices, while offline, each filled in one catalog id on a
different library label, so two labels hold one id. Don't add a picker: one id
is one label, so the write merges the two labels, keeping the smallest id.

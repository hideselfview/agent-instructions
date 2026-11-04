---
digest: Before adding UI for an edge case, make it impossible or settle it from the facts; UI only for what only the person knows, and only with their OK.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.md'
blocking: false
---

**Settle edge cases from the facts, not with special-case UI.** A conflict, an
ambiguity or a rare state is never answered by default with a screen, banner,
picker, prompt, dialog, button or extra flow. "Let the person pick" moves the
problem onto the person, and it costs a view on every platform, strings in every
locale and a state each of them has to render. Before proposing any of that, ask
in order:

1. **Can the state be made impossible?** A constraint, a single owner, an order
   of writes.
2. **Do the facts already decide it?** An id names one thing, so two rows
   holding it are one row. A rule that every device applies alike, such as the
   smallest id surviving, settles a tie with no one asked.
3. **Can the operation fail loudly to whoever started it, and be retried?**

UI is the answer only when the outcome depends on something only the person
knows: taste, intent, a fact that is not in the data. Even then it is the
person's decision, not the agent's. Raise it as a question with the facts-only
alternatives beside it, and build nothing until they choose.

Example: two library labels hold one catalog id, because two devices each filled
the id in on a different label while apart. Not a picker: one id is one label,
so the write merges the two, the smallest id surviving, as merge loops already
resolve.

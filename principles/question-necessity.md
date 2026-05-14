# Question necessity before any change

Before any change — fix, refactor, abstraction, addition — pause and ask:

- Do we need this?
- What could change so we don't need this?

The implementation reflex fires fast. The meta-question must fire first.

## Examples

- **Duplicated work across call sites.** Reflex: extract a helper. First: is the
  work even needed? If the data arrives in the shape you want (sorted, filtered,
  mapped) from upstream, the duplicated work is dead. Delete, don't extract.
- **A bug in the symptom.** Reflex: patch the symptom. First: where's the cause?
  Symptom-patches lock in the broken invariant and seed sibling bugs.
- **A new abstraction to hide a difference.** Reflex: invent `FooAdapter` to
  bridge the gap. First: is the difference real or accidental? If two things
  should be one concept, merge them upstream — don't paper over the gap with an
  adapter.
- **A new error type for an edge case.** Reflex: add `NotFoundOrStaleError`.
  First: can the type system prevent the edge case? Make the producer guarantee
  the invariant; the edge case evaporates.
- **Computing X at call sites.** Reflex: write `compute_x(input)` and call it
  from every site. First: can the producer of `input` return X directly? Often
  what every caller derives is something the source could surface once.
- **A new flag or config option.** Reflex: add the flag, expose it, branch on
  it. First: is the variation real? Or is the flag covering a transient case or
  a single use that doesn't justify the configuration surface?

## When the answer is "yes, we need this"

The check isn't a veto — it's a filter. When the change is genuinely necessary
(the duplicate isn't dead, the symptom IS the bug, the abstraction is real, the
variation is permanent), proceed. The purpose of the question is to catch the
false-need case before it costs.

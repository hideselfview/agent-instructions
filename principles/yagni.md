# YAGNI — delete to the root

You aren't gonna need it. When code is unused, remove it at the source, not at
the consumer. Filtering or guarding around dead code preserves it: the
definition still exists, the fields that existed only to model it stay optional,
and every consumer keeps the branch.

The best outcome of a YAGNI pass is: the dead definition is gone, the remaining
fields that existed only to accommodate it become required, and the branching
disappears everywhere downstream.

## Examples

- **Dead event variant.** A `Complete` event variant is emitted but no consumer
  uses it. Reflex: filter the variant in the consumer. YAGNI: delete the
  emission, delete the variant, delete the `Optional` fields that existed only
  because this variant set them to None.
- **Optional field that's always None at one call site.** Reflex: add a check at
  the call site. YAGNI: trace why it's None — if no call site ever populates it,
  delete the field. If only some do, two concepts are sharing one type; split
  them.
- **Filter on an enum match.** Reflex: skip a case in a match arm. YAGNI: ask
  why the case exists. If nothing populates it, remove the case from the enum.

## How to apply

1. Check who calls it — grep for callers of the definition, not just the visible
   path in.
2. Zero callers → delete the definition, not just the route to it.
3. Only callers are tests → delete both.
4. Don't add `Option`/`Optional`, filter, or guard to model cases that don't
   actually exist.
5. After deletion, audit field optionality: any field that was optional only to
   model a now-dead case should become required.

## Pair with question-necessity

YAGNI is the *retrospective* discipline: this code exists — is it needed?
Question-necessity is the *prospective* discipline: I'm about to add something —
is it needed?

The two reinforce each other. When you're tempted to add a wrapper, filter,
guard, or `Option` to handle some case, ask both:

- *(Question-necessity)* Do I need this at all? If the upstream producer is
  fixed, does this dissolve?
- *(YAGNI)* Is the case I'm modeling actually populated anywhere? If not, the
  right move isn't to handle it — it's to delete the source.

When both questions fire, the answer is usually "delete the dead path and
collapse the optionality it required."

See also: `principles/question-necessity.md`.

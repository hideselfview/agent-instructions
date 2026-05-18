# Compose existing primitives before adding new ones

When you need new behavior, build it by combining the pieces the system already
has. Only add a new primitive — a function, method, keyword, config flag,
endpoint, trait — after checking that the existing ones can't combine into the
answer.

A new primitive feels tidy because it collapses a multi-step combination into
one named thing. But each one:

- Hides what the existing primitives could already do.
- Bakes a specific case into a layer that was generic.
- Encourages the next similar case to get its own bespoke primitive too.

Over time those add up: a generic layer becomes a directory of special cases,
and the system's real shape gets harder to see.

## Examples

- **A built-in that bundles fetch + parse + extract for one external service.**
  Reflex: bake it into the runtime so the call site is one line. Check first:
  HTTP, JSON parsing, and field extraction already exist as primitives. The
  service-specific URL and response shape is just data — it belongs in user code
  that combines those primitives, not in the runtime.

- **A `merge(a, b)` method when records can already be merged with the existing
  field-update primitives.** Reflex: one method for the common case. Check
  first: the primitives already combine into merge. The method invites every
  variant — deep, shallow, conflict rules — to grow its own method or parameter.

- **A plugin registry generalizing a hardcoded built-in.** Reflex: when one
  built-in special case is wrong, generalize to a registry of them. Check first:
  the registry is itself a new primitive. If the existing primitives combine
  into the case, the right move is to *delete* the special case, not to
  pluginize it.

## The check before adding a primitive

1. **What primitives already exist** that touch this concern? Read the code;
   don't guess.
2. **Can they combine into what's needed?** Sketch the combination. If the
   sketch works, nothing is missing.
3. **If the combination is awkward, where?** Is the system actually missing a
   primitive (a real gap), or just missing the user code that combines the
   existing ones?
4. **Does the new primitive's name carry a domain** — a provider, ontology,
   vendor, external service? That's a tell: you're baking a specific case into
   vocabulary that should stay generic.

The check isn't a veto. Sometimes a primitive is genuinely missing. But the
default answer is *combine first, add only when the sketch really doesn't
reach*.

## Why this matters

Finding how existing primitives combine into new behavior is one of the central
activities of design. Each time you combine instead of add, you learn something
about how the system fits together. Each time you add instead of combine, you
hide that.

## Pair with question-necessity and YAGNI

- *(Question-necessity)* "Do we need this change at all?" — fires before any
  modification.
- *(Composable-primitives)* "Can we get there by combining what exists?" — fires
  when the change is needed, before reaching for a new primitive.
- *(YAGNI)* "This existing thing isn't used — delete it." — fires
  retrospectively on dead code.

The three reinforce each other. When a new feature seems to require a new
primitive, ask question-necessity first (do we even need the feature?),
composable-primitives second (if yes, can existing primitives carry it?), and
keep YAGNI ready in case the change reveals something else is now dead.

See also: `principles/yagni.md`, `principles/question-necessity.md`,
`principles/revealing-structure.md`.

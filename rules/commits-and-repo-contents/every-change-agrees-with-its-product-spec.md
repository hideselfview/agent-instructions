---
digest: Read the product spec's text on a behavior before you plan, brief, build or review a change to it. A change agrees with the spec, or changes the spec in the same commit because the person decided so; a behavior the spec doesn't state is added to it in the same commit.
paths:
  - '**/*'
blocking: true
---

## Every change agrees with its product spec

A project's product spec (bae: `spec/bae.md`; coven: `spec/coven.md` and
`spec/api.md`) states what the product does. The person decided each behavior it
states, often after going back and forth, so the spec outranks a brief, a plan,
an item's text, a rule's general guidance and the code.

**Planning and briefs.** Before you write a plan or a brief, or settle an open
choice an agent raises, search the spec for the behavior the change touches and
read the whole section around each match. A brief points at that spec text
instead of restating the behavior. When a rule or a fact seems to settle a
choice, check the spec first: if the spec states the behavior, follow it. A
change that would make the product behave differently from the spec is the
person's decision. Raise it, and build nothing until they choose.

**Building.** Read the spec's text on each behavior you change before you change
it. When the brief and the spec disagree, stop and report both texts.

**Review.** Flag each change that:

- makes the product behave differently from what the spec states, unless the
  same commit edits the spec to say so;
- adds behavior a person can see, or a schema or sync shape, that the spec
  doesn't state, unless the same commit adds it to the spec;
- edits the spec so that it contradicts another part of the spec.

Quote the spec text with its file and line.

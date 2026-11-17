---
digest: Read the product spec's text on a behavior before you plan, brief, build or review a change to it. A change agrees with the spec, or changes the spec in the same commit because the person decided so, and adds any behavior the spec doesn't state to the spec in the same commit. When work finds the spec contradicting a rule or what the change builds, the same work edits the spec and names that edit in its commit message and in its report to the person. A question to the person about spec text that looks wrong says so and offers the fix.
paths:
  - '**/*'
blocking: true
---

## Every change agrees with its product spec

A project's product spec (bae: `spec/bae.md`; coven: `spec/coven.md` and
`spec/api.md`) states what the product does. The person decided most of the
behaviors it states, often after going back and forth, so a plan or a brief
follows the spec's text over a rule's general guidance, an item's text and the
code.

The spec is also a work in progress, and some of its text is wrong: the text
contradicts a rule, another part of the spec, or what a change builds. Work that
finds such a contradiction edits the spec and names the edit to the person, so
that each change leaves the spec and the product in agreement and the person
sees every spec edit.

**Planning and briefs.** Before you write a plan or a brief, or settle an open
choice an agent raises, search the spec for the behavior the change touches and
read the whole section around each match. A brief points at that spec text
instead of restating the behavior. When a rule's general guidance or a fact
seems to settle a choice, check the spec first: if the spec states the behavior,
follow it. A change that would make the product behave differently from the spec
is the person's decision. Raise it, and build nothing until they choose. When
the spec text looks wrong, the question to the person says so, names the rule or
the spec text it contradicts, and offers the spec edit that fixes it. Never
present spec text that looks wrong as a decision the person already made.

**Building.** Read the spec's text on each behavior you change before you change
it. When the spec contradicts a rule or what the change builds, edit the spec in
the same commit so that it agrees with the rule and states what the change
builds. A commit that changes or removes spec text names that edit in its
message, and your report names it to the person with the old and the new text.

**Review.** Flag each change that:

- makes the product behave differently from what the spec states, unless the
  same commit edits the spec to say so;
- adds behavior a person can see, or a schema or sync shape, that the spec
  doesn't state, unless the same commit adds it to the spec;
- edits the spec so that it contradicts another part of the spec;
- changes or removes spec text in a commit whose message doesn't name that edit.

Quote the spec text with its file and line.

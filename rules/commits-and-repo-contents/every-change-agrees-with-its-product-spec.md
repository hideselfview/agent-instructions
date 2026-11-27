---
paths:
  - '**/*'
blocking: true
---

## Every change agrees with its product spec

A project's product spec (bae: `spec/bae.md`; coven: `spec/coven.md` and
`spec/api.md`) states what the product does. The spec is a draft: its text
changes often, and some of it is wrong. The spec records what the person has
said so far and is the one document that states each behavior, so every plan,
brief and change reads it first. No agent treats spec text as a decision the
person already made.

The coding rules outrank the spec. When the spec states a shape or a behavior
that a rule forbids, the rule holds, and the spec text is a mistake.

**Changing the spec is part of the work.** A spec edit for work the person asked
for or decided needs no question: the agent makes it in the commit that does the
work, whether the spec stated nothing on the behavior or stated it otherwise.
When such work and the spec differ, the spec changes. Only a change nobody
decided goes to the person, such as a contradiction the agent finds that its
work doesn't ask it to settle, or a product choice that neither the person's
request nor a decision made. For such a change, the agent asks the person,
quotes the spec text with its file and line, says why the text looks wrong, and
offers the edit. The agent edits the spec only after the person agrees, in the
commit that builds the change.

**A detail the code already settles.** When working code already does something
sensible and the spec differs only on a detail, such as how the person reaches
the same outcome, the difference is not the person's question: the spec is a
draft and fell behind. The code holds, and the agent edits the spec to say what
the code does, in the commit it is making or one of its own, and asks no one.
Which screen a button opens, when both screens let the person do the same thing,
is such a detail: a button that opens the restore screen, where the person
pastes or scans the code, settles a spec line that says it opens the camera. A
difference that nobody decided in what the person can do, what they keep or
lose, or what a screen tells them is theirs to decide.

**Storage the work needs.** A table, a column, an index or a synced field that
the work needs is how the product does what the person decided, not a decision
of its own. When the spec's schema text, such as a table's row or a sync shape,
does not list that storage, the spec states nothing on it: the commit adds the
storage to the spec, and no one asks the person. Storage conflicts with the spec
only when the spec's text says otherwise, such as a table's row that says the
table does not store that value.

**Planning and briefs.** Before you write a plan or a brief, or settle an open
choice an agent raises, search the spec for the behavior the change touches and
read the whole section around each match. A brief points at that spec text
instead of restating the behavior. A plan follows the spec on each behavior that
its work doesn't change. Where work the person asked for or decided changes a
behavior the spec states, or a rule forbids what the spec states, the plan
follows the work or the rule and names the spec edit. Any other change to a
behavior the spec states is the person's decision, as is what the product does
instead of a forbidden behavior when neither the rule nor a decision says,
except a detail the code already settles (above): raise it, and build nothing
until they choose.

**Building.** Read the spec's text on each behavior you change before you change
it, and edit the spec in the same commit wherever your work changes what the
spec says. Before you commit, ask the person about each contradiction whose
resolution nobody decided, except a detail the code already settles, and make
that edit once they agree. A commit that changes or removes spec text names that
edit in its message, and your report names it to the person with the old and the
new text.

**Review.** Flag each change that:

- makes the product behave differently from what the spec states, unless the
  same commit edits the spec to say so;
- follows spec text that a rule forbids, instead of the rule;
- adds behavior a person can see, or a schema or sync shape, that the spec
  doesn't state, unless the same commit adds it to the spec;
- edits the spec so that it contradicts another part of the spec;
- changes or removes spec text in a commit whose message doesn't name that edit;
- changes or removes spec text that neither the brief's work calls for nor the
  code already settles, unless the brief says the person agreed to the edit.

Quote the spec text with its file and line.

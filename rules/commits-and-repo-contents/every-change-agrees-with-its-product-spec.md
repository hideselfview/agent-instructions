---
digest: The product spec is a draft that the coding rules outrank. Read the spec's text on a behavior before you plan, brief, build or review a change to it, and never treat that text as a decision the person made. When working code already does something sensible and the spec differs only on a detail, the code holds and the agent edits the spec without asking. When the spec contradicts a rule, the code in a way the person would notice, or what a change builds, ask the person, say which text looks wrong and why, offer the edit, and edit the spec only after they agree. A commit adds to the spec what it builds where the spec states nothing or the person already decided.
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
that a rule forbids, the rule holds, and the spec text is a mistake to raise
with the person.

The person decides every change to what the spec says. When work finds the spec
contradicting a rule, another part of the spec, the code or what the change
builds, the agent asks the person, quotes the spec text with its file and line,
says why the text looks wrong, and offers the edit. The agent edits the spec
only after the person agrees, in the commit that builds the change. A spec edit
that only records what a commit builds, where the spec stated nothing or the
person already decided, needs no question and goes in that commit.

**A detail the code already settles.** When working code already does something
sensible and the spec differs only on a detail, such as how the person reaches
the same outcome, the difference is not the person's question: the spec is a
draft and fell behind. The code holds, and the agent edits the spec to say what
the code does, in the commit it is making or one of its own, and asks no one.
Which screen a button opens, when both screens let the person do the same thing,
is such a detail: a button that opens the restore screen, where the person
pastes or scans the code, settles a spec line that says it opens the camera. A
difference in what the person can do, what they keep or lose, or what a screen
tells them is theirs to decide.

**Planning and briefs.** Before you write a plan or a brief, or settle an open
choice an agent raises, search the spec for the behavior the change touches and
read the whole section around each match. A brief points at that spec text
instead of restating the behavior. When the spec states a behavior and nothing
contradicts it, the plan follows the spec. When a rule forbids what the spec
states, the plan follows the rule, and the question to the person offers the
spec edit. Any other change to a behavior the spec states is the person's
decision, except a detail the code already settles (above): raise it, and build
nothing until they choose.

**Building.** Read the spec's text on each behavior you change before you change
it. Ask the person about each contradiction you find before you commit, except a
detail the code already settles, and edit the spec in the same commit once they
agree. A commit that changes or removes spec text names that edit in its
message, and your report names it to the person with the old and the new text.

**Review.** Flag each change that:

- makes the product behave differently from what the spec states, unless the
  same commit edits the spec to say so;
- follows spec text that a rule forbids, instead of the rule;
- adds behavior a person can see, or a schema or sync shape, that the spec
  doesn't state, unless the same commit adds it to the spec;
- edits the spec so that it contradicts another part of the spec;
- changes or removes spec text in a commit whose message doesn't name that edit,
  or whose brief doesn't say the person agreed to it.

Quote the spec text with its file and line.

---
digest: Correct, well-tested code can still be dead weight; a mechanism is justified only if deleting it breaks an outcome a user would notice - check by stubbing it out and running the full suite, not by its own passing tests.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
review: false
blocking: false
---

## A mechanism must change an outcome

A mechanism can be correct, called and fully tested, and still change no
decision that matters: nothing uses its result, another mechanism already covers
the outcome, or it is connected to a different part of the system than the one
it was built for. Review of one diff, or of whether each unit is correct, misses
this, because no single file or diff holds the contradiction; it appears across
changes, and sometimes across repos.

A mechanism's tests of itself don't show it is needed. Each asserts the
mechanism's output against its input; none asserts an outcome that depends on
it. When a mechanism's only tests are tests of itself, its necessity is
unverified, and the missing test of an outside outcome is itself the finding.

### Stub it out and see what breaks

Remove the mechanism, or stub it (return a constant, make the call do nothing),
and run the full suite:

- Only its own unit tests break, or nothing outside its file: it is orphaned.
- A behavioral, integration or end-to-end assertion about a real outcome breaks:
  it changes an outcome.

Never take a mechanism's purpose from its comment. A doc comment states the
intended design, and the wiring may have moved away from it. Trace the
definition to its producers, its consumers and the decision it changes at run
time.

### Kinds of finding

- Orphaned: it changes no decision.
- Comment drift: the comment describes the plan, and the wiring differs.
- Wrong connection: built for X, connected to Y, while X runs on something
  cruder.
- Self-asserted invariant: a check that trusts input an attacker or the author
  chooses to enforce a property.
- Redundant: another mechanism fully covers the outcome.
- Cross-repo drift: one side defines a meaning, and the other fills it
  differently or not at all.

Before you flag a mechanism, argue the strongest case that it is needed, and
keep the finding only if that case fails. Removing something that is needed is
as wrong as keeping something that isn't.

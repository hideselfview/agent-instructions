---
digest: Locally-correct, well-tested code can still be slop; a mechanism is justified only if deleting it breaks a user-meaningful outcome — verify by sabotage, not by its own green tests.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
review: false
blocking: false
---

**A mechanism must be load-bearing.** Verify it by sabotage, not by its own
green tests. The dangerous slop isn't broken code; it's code that is locally
correct, called, and fully tested but **drives no decision that matters**. A
mechanism can be a correct implementation of its own spec, with green unit tests
that all "make sense," and still wire into nothing — orphaned, redundant with
another mechanism, or connected to the wrong plane while the plane it was
designed for runs on something cruder. This rot is invisible to diff-scoped
review and to "is each unit correct?" review, because no single file or diff
contains the contradiction — it's emergent across PRs (and sometimes across
repos).

**Passing self-tests are camouflage, not validation.** A unit test that mints
`t1, t2, t3` and asserts `t1 < t2 < t3` proves the clock is a correct clock — it
is *tautological* with respect to necessity. Every assertion is output-vs-input
of the mechanism itself; none asserts a user-meaningful outcome that *depends*
on the mechanism. The more thorough such self-tests are, the more cared-for and
intentional a dead mechanism looks. When a mechanism's only tests are tests of
itself against its own spec, treat its necessity as **unverified by
construction** — and that absence of an external-outcome test is itself the
finding.

**The load-bearing test (the question that actually decides):** neuter or delete
the mechanism and ask *what user-meaningful behavior breaks?* The test suite is
the dependency graph — so the mechanical version is: stub the mechanism (return
a constant, no-op the call) and run the full suite.

- Only its own unit tests / nothing outside its own file breaks → **orphan**.
- A behavioral / integration / end-to-end assertion about a real outcome breaks
  → load-bearing.

**Never assert a *purpose* from a comment.** A doc comment states the *intended*
design; the wiring may have diverged, leaving the comment a fossil. Verify a
mechanism's purpose by tracing definition → producers → consumers → the concrete
runtime decision it changes — never by repeating what the comment claims. "Never
guess" applies to purpose, not just behavior. (Worked example: an HLC whose doc
said it was the `_updated_at` LWW register, while the host stamped `_updated_at`
with a wall clock and the HLC's only live use was a spoofable, self-signed
authorization timestamp already covered by key rotation. Every file correct;
every test green; the mechanism load-bearing nowhere.)

**Classify what you find:** *orphaned* (drives no decision), *intent/doc drift*
(comment describes the plan, wiring diverged), *wrong-plane wiring* (built for
X, connected to Y, X runs on something cruder), *self-asserted invariant* (a
check that trusts attacker-or-author-chosen input to enforce a property),
*redundant* (another mechanism already fully covers the outcome), *cross-repo
contract drift* (one side defines a meaning, the other fills it differently or
not at all).

**Adversarially defend before flagging.** For each candidate, argue the
*strongest case that it IS load-bearing / necessary* first; keep the finding
only if that case fails. A false positive — ripping out something genuinely
necessary — is as bad as the slop. This is the retroactive, audit-time form of
*Question necessity before any change* and *YAGNI*: run "do we need this?"
against code that already exists, with the sabotage test as the arbiter.

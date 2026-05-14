# Revealing structure IS development

Development is the process of identifying, modeling, and realizing the structure
of the system. Not structure for the sake of structure — structure for the sake
of having a *system*. Any interesting, worthwhile system has structure with lots
and lots of details. The failure mode is never "too much detail" — it's failing
to discover or realize the details the system actually has.

Missing, unrealized, or unimplemented structure is the foremost cause of
applications that don't feel right or feel like they have something missing. It
also produces stratified abstractions, sibling bugs, and design debt. Two
implementations that produce the same behavior are not equally valuable — the
one that fully realizes the structure is strictly better.

## Structure vs human-ease

Every design decision asks: does this reveal the real shape or obscure it? Pick
the revealing option. Failure cuts both ways:

- **Adding structure that isn't really there.** Speculative abstractions,
  compound types invented before patterns prove themselves, sugar helpers that
  conceal primitive operations.
- **Failing to discover/realize structure that is there.** Conflated concepts,
  "good enough" approximations, deferred details, single types doing the work of
  two or three genuinely distinct things.

Both obscure the shape. When a human-easy fix conflicts with revealing real
structure (in either direction), prioritize structure. Human-ease is for humans;
for you the work is cheap, which is the whole point. When ease and structure
align, fine — but ease is never the justification.

## Examples

- **Wrapper chain.** Each wrapper either represents a distinct state with
  different invariants (`Request → AuthorizedRequest → ValidatedRequest`) or
  it's a decorative rename. Keep the meaningful; collapse the decorative.
- **Compound vs separate types.** Splitting reveals when distinct concepts are
  sharing one container. Merging hides ownership.
- **Primitives first, sugar later.** Sugar invented before patterns are real
  hides the shape; sugar added after they're proven reveals it.
- **Root vs symptom.** Symptom-patches hide why the symptom appeared and seed
  sibling bugs. Fix the cause.
- **One parameter per concept.** Two counts named "count, count2" obscure which
  is which. Separate by meaning, not by index.
- **Test the contract, not the behavior.** Assert what the structure guarantees,
  not what the current code happens to output. Observation tests ossify today's
  shape.

## Failure modes

- **Tracking size metrics** (line count, file count, "how verbose now") as
  proxies. Smaller often means better-hidden, not better-structured. Ask "is the
  shape more visible?", not "how many lines?".
- **Treating discovered detail as bloat.** When the system genuinely has three
  concepts, modeling them as three types isn't bloat — it's revealing structure.
  Pushing back on "too many types" because the model feels verbose is rejecting
  the actual shape.
- **Rushing to re-wrap bare primitive forms** because they look ugly. The
  verbosity is the structure showing through; sugar puts the cover back on.
- **Keeping sugar "because the common case reads better."** The common case's
  readability comes from the structure being clear, not from sugar covering it.
- **Presenting the reveal as a cost.** ("This will be a bigger diff but…") —
  drop the "but". The diff is not a cost; the reveal is the point.
- **Asking for measurements instead of observations.** "Line-count before vs
  after" → wrong. "What pattern recurred in the rewrite?" → right.

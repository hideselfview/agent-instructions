# Revealing structure IS development

Development is the process of identifying, modeling, and realizing the
structure of the system. Not structure for the sake of structure —
structure for the sake of having a *system*. Any interesting, worthwhile
system has structure with lots and lots of details. The failure mode is
never "too much detail" — it's failing to discover or realize the details
the system actually has.

Missing, unrealized, or unimplemented structure is the root cause of
bugs, stratified abstractions, sibling bugs, and design debt. Two
implementations that produce the same behavior are not equally valuable
— the one that fully realizes the structure is strictly better.

## Structure vs human-ease

Every design decision asks: does this reveal the real shape or obscure
it? Pick the revealing option. Failure cuts both ways:

- **Adding structure that isn't really there.** Speculative abstractions,
  compound types invented before patterns prove themselves, sugar
  helpers that conceal primitive operations.
- **Failing to discover/realize structure that is there.** Conflated
  concepts, "good enough" approximations, deferred details, single types
  doing the work of two or three genuinely distinct things.

Both obscure the shape. When a human-easy fix conflicts with revealing
real structure (in either direction), prioritize structure. Human-ease is
for humans; for you the work is cheap, which is the whole point. When
ease and structure align, fine — but ease is never the justification.

## Examples

- **Three intermediate abstractions vs one primitive.** If the three are
  speculative or duplicate, collapse. If each represents a genuine
  distinct concept, keep them and surface their distinctness.
- **Compound struct conflating distinct concepts vs separate types.**
  Separate types reveal structure when the concepts are actually
  distinct.
- **Sugar helpers invented preemptively vs primitives first.**
  Primitives-first reveals structure; preemptive sugar conceals the real
  shape until patterns prove themselves.
- **A fix that suppresses a symptom vs one that addresses the root.** The
  root fix reveals structure; the symptom-fix hides it.
- **An abstraction parameter that conflates two distinct things vs
  separate parameters per concept.** Separate reveals structure;
  conflated obscures ownership.
- **A test that pins observed behavior vs intended structure.**
  Intended-structure reveals; observed-behavior ossifies the current
  shape.

## Failure modes

- **Tracking size metrics** (line count, file count, "how verbose now")
  as proxies. They obscure structure rather than measure it.
- **Treating discovered detail as bloat.** When the system genuinely has
  three concepts, modeling them as three types isn't bloat — it's
  revealing structure. Pushing back on "too many types" because the
  model feels verbose is rejecting the actual shape.
- **Rushing to re-wrap bare primitive forms** because they look ugly. The
  verbosity is the structure showing through; sugar puts the cover back
  on.
- **Keeping sugar "because the common case reads better."** The common
  case's readability comes from the structure being clear, not from sugar
  covering it.
- **Presenting the reveal as a cost.** ("This will be a bigger diff
  but…") — drop the "but". The diff is not a cost; the reveal is the
  point.
- **Asking for measurements instead of observations.** "Line-count before
  vs after" → wrong. "What pattern recurred in the rewrite?" → right.

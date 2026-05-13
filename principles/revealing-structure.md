# Revealing structure IS development

The entire development process is an effort to reveal the underlying
structure of the system. The sooner the structure is revealed, the sooner
the work is done. Two implementations that produce the same behavior are
not equally valuable — the one that reveals structure is strictly better.
Hidden structure is the root cause of bugs, stratified abstractions,
sibling bugs, and design debt.

## Structure vs human-ease

Every design decision asks: does this reveal structure or obscure it?
Pick the revealing option. When a human-easy fix conflicts with revealing
structure, prioritize structure. Human-ease is for humans; for you the
work is cheap, which is the whole point. When ease and structure align,
fine — but ease is never the justification.

## Examples

- **Three intermediate abstractions vs one primitive.** Primitive reveals
  structure. Collapse.
- **Compound struct with overlapping fields vs orthogonal types.**
  Orthogonal reveals structure. Split.
- **Sugar helpers invented preemptively vs primitives first.**
  Primitives-first reveals structure; preemptive sugar hides it until the
  patterns prove themselves.
- **A fix that suppresses a symptom vs a fix that addresses the root.**
  Root reveals structure; symptom-fix hides it.
- **An abstraction parameter that takes two counts for the same thing vs
  one.** One count reveals structure; two obscures ownership.
- **A test that pins observed behavior vs intended structure.**
  Intended-structure reveals; observed-behavior ossifies the current
  shape.

## Failure modes

- **Tracking size metrics** (line count, file count, "how verbose now")
  as proxies. They obscure structure rather than measure it.
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

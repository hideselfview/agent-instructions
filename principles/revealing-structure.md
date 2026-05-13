# Revealing structure IS development

Not a phase, not a style, not a nice-to-have. The entire development process
is an effort to reveal the underlying structure of the system. The sooner
the structure is revealed, the sooner the work is done.

This is the meta-principle that drives every other rule. Other rules
(maximalism, no-hedge-on-diff-size, no-"simpler first step",
composition-over-stratification) are all consequences of this one.

## Why this is THE principle

- You can't write correct code against a structure you can't see. Hidden
  structure is the root cause of bugs, stratified abstractions, sibling
  bugs, and design debt.
- Every hour spent hiding structure (through hedging, sugar-too-early,
  compound abstractions, deferred refactors) is an hour added to the
  eventual reveal.
- Every hour spent revealing structure — even if it looks like "just a
  refactor" — is direct progress toward a finished system.
- Two implementations that produce the same behavior are not equally
  valuable: the one that reveals structure is strictly better.
  Structure-revealing code is easier to extend, easier to debug, and
  easier to reason about.

## How to apply — always, to every decision

Before every meaningful choice, ask: **does this reveal structure or
obscure it?** Pick the revealing option.

Examples:

- **Three intermediate abstractions vs one primitive.** The primitive
  reveals structure. Collapse.
- **Compound struct with overlapping fields vs orthogonal types.**
  Orthogonal reveals structure. Split.
- **Sugar helpers invented preemptively vs primitives first, sugar where
  patterns justify.** Primitives-first reveals structure; speculative
  sugar obscures it until the patterns are known.
- **A fix that suppresses a symptom vs a fix that addresses the root.**
  The root reveals structure; the symptom-fix hides it.
- **An abstraction parameter that takes two counts for the same thing vs
  one count.** One count reveals structure; two obscures which owns what.
- **A test that pins observed behavior vs a test that pins intended
  structure.** Intended-structure reveals structure; observed-behavior
  ossifies the current shape.

## What this means in practice

- **Don't hedge on the size of structural work.** Diff size, scope, and
  effort are irrelevant when the outcome reveals structure. Revealing
  structure IS the goal.
- **Simplification is the main mode, not an occasional one.** Most of the
  time, the right move is "collapse this to see it clearly."
- **Bare primitive forms are informative on purpose.** When a refactor
  produces a verbose bare state, that verbosity is showing you the real
  shape of the problem. Don't rush to re-wrap it.
- **Premature abstractions are a form of obscurement.** Extract helpers
  only after repeated patterns prove they're stable. Sugar added too
  early hides structure that hasn't finished emerging.
- **Tests should pin structure, not behavior-as-observed.** A test that
  asserts "this is how the code happens to act today" freezes the current
  shape. A test that asserts "this is the contract the structure
  guarantees" reveals the structure.
- **Every report, every status update, every agent deliverable should
  center structural observations.** Which patterns recurred? Which
  boundaries held up? Which abstractions turned out to be false? Not:
  how many lines, how many files, how long did it take.

## Failure modes — watch for these patterns in yourself

- Tracking size metrics (line count, file count, "how verbose now") as
  proxies. They're not proxies for anything real. They obscure structure.
- Proposing to keep structure-hiding sugar "because the common case reads
  better." The common case's readability is a function of the structure
  being clear, not of the sugar covering the structure.
- Presenting the reveal as a cost. ("This will be a bigger diff but…") —
  drop the "but". The diff is not a cost; the reveal is the point.
- Asking for observations in terms that invite the agent to measure
  instead of observe. ("Line-count before vs after" → wrong. "What
  pattern recurred in the rewritten fixture?" → right.)

---
name: cleanup-hunt
description: Hunt a codebase for trash — dead code, copy-paste implementations, unneeded abstractions, missing abstractions whose absence causes duplication — and land verified deletions in small receipted batches. Use when asked to clean up a codebase, find dead code, hunt duplication, reduce line count, or run a cleanup sweep.
---

# Cleanup hunt

Find and remove what shouldn't exist: dead code, repeated implementations,
abstractions with no consumer, and duplication caused by a missing shared piece.
Every cut is a claim about the world and needs a receipt. The output is merged
batches plus a leave-alone ledger — declining a cut with a recorded reason is
output, not failure.

## The core distinction: duplication vs structural parallelism

Scanning finds *similar shape*; only reading finds *sameness*. Expect every
scanner estimate to deflate 40–80% on reading, because the residue left after
two operations' shared steps are factored is usually the domain's real
differences, not duplication. Two providers' access-control bodies that would
need eight parameters to unify — where the parameters ARE the providers'
differences — are structural parallelism. Leave them, and record why.

Signals that a collapse is real:

1. **An already-factored target shape exists in the codebase.** Converging
   copies onto a helper the code already has is the reliable case; inventing a
   new abstraction to force-unify is the unreliable one.
2. **The variation is representation, not meaning.** A forwarding shim, a
   byte-identical block, a literal that differs — these carry no domain
   vocabulary that could make them irreducible.
3. **A rule is stated twice.** Two implementations of the same protocol
   invariant (not two operations that share steps) must collapse even when the
   line count is neutral — the value is one statement per rule, and divergence
   is a future bug. Say the number honestly and argue the merit separately.

False-duplicate tells: same name but different recorded facts per user; a "copy"
whose extra field or stricter predicate is load-bearing; per-domain error
vocabulary. When a shared helper would need a parameter with no meaning, stop.

## Where dead code hides from the lint

`dead_code` (and its analogues) has blind spots; hunt them by hand:

- **`pub` surface.** The lint is off for exported items. Before gating or
  deleting, grep every sibling/consumer repo and cite the grep in the commit
  message. Non-Rust hosts (Swift, Kotlin, C#) don't show up in a Rust grep —
  documented host API with zero Rust callers is a report, not a cut.
- **Closed reference cycles.** A `pub` trait's dead methods can be the only
  callers of `pub(crate)` functions; each dead link keeps the next one "used".
  Trace reachability from live roots, not reference counts.
- **Narrowing re-arms the lint.** Tightening `pub` → `pub(crate)` turns
  detection back on for the whole chain below. After gating one item, expect the
  compiler to reveal the next dead layer — follow the cascade to where real
  callers exist.
- **Superseded designs.** A half-built-looking lifecycle may be a *pending* step
  or a *superseded* one — the code can't tell you which; plans, ratified
  deviations, and commit history can. Cite the superseding decision when
  cutting.

## Receipts

Every claim gets the check that would catch it being wrong:

- **Deletion** → grep-zero on the name (plus sibling repos for `pub`), build,
  full suite.
- **Behavior preserved** → the targeted suite for every touched area, plus
  clippy/lint on all targets and every feature combination that compiles the
  code (a collapse once broke default-feature builds while the all-features CI
  stayed green).
- **A guard or mechanism is load-bearing** → sabotage it (delete the check) and
  watch a test go red; if nothing reds, that absence is itself a finding.
- **A bug found while reading** → failing test first, proving the defect; then
  fix; then green. Never "the fix is X" before there's a red test.
- **Merging onto a moving or already-red main** → failure-set identity: run the
  suite at your tip and at the base tip, diff the failing-test name sets. Your
  batch must add zero. Don't re-verify a moving tip forever — verify once, then
  rebase-and-push immediately; CI is the net.

## Batch discipline

- One branch per batch off the current base; one commit per concern; ff-only
  merges, rebased first.
- Read the pattern across the whole crate **before** choosing an extracted
  helper's visibility — under-scoping a helper creates the next batch's
  duplication.
- Never script mass edits across many sites; hand-edit with the compiler in the
  loop. Undo experiments with the inverse edit, never a checkout that can
  destroy uncommitted work.
- Measure line count from git objects
  (`git grep -c '' <ref> -- '*.rs' | awk -F: '{s+=$NF} END {print s}'`), not
  from a working tree — other sessions' uncommitted edits pollute tree counts.
  Report the delta per batch.
- When other agents or sessions share the repo, keep a claims map: which files
  each party is editing. Findings in someone's active area are reported, not
  cut, until their tree quiets; re-check what's freed each round.

## What outranks the deletions

The best finds are not line count:

- A "duplicate" test that is actually an **untested scenario** — its doc
  describes a behavior its body never creates. Deleting it would bless a
  coverage gap; the fix is making the test do what it says, and the failing run
  tells you which mechanism actually enforces the invariant.
- A copy that **dropped a validation** the original performs — a dormant bug
  wearing duplication's clothes.
- Dead fields reported in errors, write-only tables shipped to peers, retry
  loops that can never succeed. Ask *why* two things differ before making them
  the same; the answer is sometimes a defect.

## Report format

Per batch: commits with one-line whats, line-count delta (git-object count),
receipts run, deliberate leave-alones with reasons, and report-only findings
that need an owner's decision. Keep the leave-alone ledger cumulative so the
next pass doesn't re-litigate settled declines.

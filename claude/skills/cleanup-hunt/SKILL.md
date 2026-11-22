---
name: cleanup-hunt
description: Find and remove dead code, copy-paste implementations, unneeded abstractions, and duplication that a missing shared piece causes, and recompose structure such as side effects mixed into decisions, pass-through layers and mode flags. Lands verified changes in small batches, each with the checks that prove it. Use when asked to clean up a codebase, find dead code, hunt duplication, reduce line count, compose out side effects, or run a cleanup sweep.
---

# Cleanup hunt

Find and remove what shouldn't exist: dead code, repeated implementations,
abstractions with no consumer, and duplication that a missing shared piece
causes. Every cut claims something about the code, and each claim needs the
check that would catch it being wrong. The output is landed batches plus a list
of the cuts you declined and why: a declined cut with its reason is a result,
not a failure.

Run `builds brief [--project P] PATH...` to read the rules for the files you
inspect.

## Duplication versus structural parallelism

Scanning finds similar shapes; only reading finds code that is the same. Expect
reading to rule out 40–80% of what a scanner reports, because what is left after
two operations' shared steps are factored out is usually the domain's real
differences. Two providers' access-control bodies that would need eight
parameters to unify, where the parameters are the providers' differences, are
structural parallelism: leave them, and record why.

These signs show that a collapse is real:

1. **The codebase already has the shared shape.**
   - Moving copies onto a helper the code already has is the reliable case.
   - Inventing a new abstraction to force them together is the unreliable one.
2. **The copies differ in representation, not meaning.**
   - A forwarding shim, a byte-identical block or a different literal carries no
     domain words that could make it irreducible.
3. **One rule is stated twice.** Two implementations of the same invariant (not
   two operations that share steps) must collapse.
   - They collapse even when the line count stays the same: each rule gets one
     statement, and two copies diverge into a bug.
   - Report the line count as it is, and argue the merit separately.

These signs show a false duplicate: the same name over different facts per user,
a copy whose extra field or stricter check matters, and error words that belong
to each domain. When a shared helper would need a parameter with no meaning,
stop.

## Dead code the lint misses

`dead_code` and the lints like it in other languages miss these cases; find them
by hand:

- **`pub` items.** The lint ignores exported items.
  - Before you narrow or delete one, grep every sibling and consumer repo, and
    cite the grep in the commit message.
  - Swift, Kotlin and C# callers don't show up in a Rust grep: a documented host
    API with no Rust callers is a report, not a cut.
- **Closed cycles of references.** Each dead link can keep the next one in use.
  - A `pub` trait's dead methods can be the only callers of `pub(crate)`
    functions.
  - Trace what live entry points reach, not how many references an item has.
- **Narrowed visibility.** Changing `pub` to `pub(crate)` turns the lint back on
  for the whole chain below.
  - After you narrow one item, expect the compiler to show the next dead layer,
    and follow it down to where real callers exist.
- **Superseded designs.** A lifecycle that looks half-built may be a step still
  to come or one a later design replaced.
  - The code can't tell you which; plans, recorded decisions and commit history
    can.
  - Cite the decision that replaced it when you cut it.

## Checks for each claim

Every claim gets the check that would catch it being wrong:

- **A deletion:** no grep match for the name (in sibling repos too, for `pub`),
  a build, and the tests of the crates the change touches.
- **Unchanged behavior:** the tests of every touched area, plus clippy or the
  linter on all targets and on every feature combination that compiles the code.
  - A change can break the default-feature build while the all-features build
    passes.
- **A guard or mechanism is needed:** remove it and watch a test fail, as the
  rule that a mechanism must be load-bearing says.
  - When no test fails, that is a finding.
- **A bug you find while reading:** fix it as the bug-fix rule says, starting
  with a test that fails because of the bug.
- **A main that moves or already fails tests:** compare sets of failing tests.
  - Run the touched tests at your tip and at the base, and compare the names
    that fail: your batch must add none.
  - Check once, then land at once. Don't check again every time main moves; CI
    runs every suite on main after you land.

## Batches

- Start one branch per batch from the current base, and land it as
  `builds help workflow` describes.
- Read the pattern across the whole crate before you choose an extracted
  helper's visibility: a helper scoped too narrowly makes the next batch's
  duplication.
- Edit each site by hand and build after each edit, never with a script across
  many sites.
  - Undo an experiment with the inverse edit, never with a checkout that can
    destroy uncommitted work.
- Count lines from git objects, not from a working tree, which holds other
  sessions' uncommitted edits.
  - `git grep -c '' <ref> -- '*.rs' | awk -F: '{s+=$NF} END {print s}'`
  - Report the change in line count for each batch.
- When other sessions share the repo, keep a list of the files each one edits.
  - Report findings in a file another session is editing; don't cut them until
    that session stops editing it.
  - Check the list again each round.

## Structure to recompose

Some shapes need recomposing, not removing:

- **Side effects mixed into decisions.** A function that decides what to do and
  also does it can't be tested, reused or understood without the effect.
  - Move the effect out: a pure function returns the decision (a plan, a value,
    an enum), and the caller performs the write, the send or the delete.
  - The sign is a test that needs a fake filesystem or network to check what is
    arithmetic or matching.
- **Ambient dependencies.** A clock, RNG, environment read or filesystem probe
  called in the middle of a function is a hidden input. Fix it as the dependency
  injection rule says.
- **Reads that write.** A getter that also writes hides a state change behind a
  read.
  - Examples are a getter that caches, repairs or moves a cursor.
  - Split it so the caller sees both operations.
- **Mode flags.** A boolean or enum parameter that switches a function between
  two mostly separate bodies makes two functions that share a name.
  - Split it at the call sites, and keep the steps they really share in helpers.
- **Layer leaks.** A module that reaches past its boundary shows a missing
  boundary or an unneeded layer.
  - Examples are SQL in a facade, a provider name in generic code, and wire
    types in domain logic.
  - Decide which of the two it shows, and fix the leak.
- **Pass-through layers.** A type or module whose every member forwards to
  another goes, as the rule that structure follows the domain says.
  - Keep it when a boundary such as FFI, a wire format or API stability needs
    the copy.

Recomposing changes shape, not behavior, so its check is the existing tests,
unchanged. Where the mixed version had no tests because it couldn't, the first
test of the pure function is the reason for the change.

## Finds that matter more than deleted lines

The best finds don't reduce the line count:

- A "duplicate" test whose doc describes a case its body never creates is an
  untested case.
  - Deleting it would hide the gap. Make the test do what its doc says; its
    failing run shows which mechanism really enforces the invariant.
- A copy that dropped a check the original makes is a bug that looks like
  duplication.
- Dead fields reported in errors, write-only tables sent to other devices, and
  retry loops that can never succeed belong in this list too.
- Ask why two things differ before you make them the same: sometimes the answer
  is a defect.

## Report

For each batch, report:

- each commit, with one line on what it does
- the change in line count, counted from git objects
- the checks you ran
- the cuts you declined, with reasons
- findings that need an owner's decision; dead code is never one of them
  (`write-only-what-is-needed-today.md`, Dead code)

Keep the list of declined cuts across batches, so the next pass doesn't argue
them again.

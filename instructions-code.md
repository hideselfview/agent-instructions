# Writing style

**No marketing language anywhere.** Applies to all written artifacts — code,
comments, docs, notes, UI text, error messages, commit messages, PR descriptions
— and to conversational explanations to the user. No "improves user experience,"
"seamless," "powerful," "elegant," "robust," "intuitive," "delightful," etc.
State what it does plainly. *(Pairs with "No pitch-deck framing" in
`instructions-agent.md` — that rule covers strategy/business vocabulary
specifically.)*

**Commit messages: brief, why not what.** No padding, no celebrating the change.
Focus on *why* (when not obvious from the diff) — the diff already shows *what*.

**PR descriptions: narrative prose, not changelogs.** Explain the situation, the
problem, and the approach in a few sentences. The diff shows *what* changed —
the PR body explains *why*. No bullet lists of "changed X to Y in file Z" — that
duplicates the diff. Test plan section can stay as bullets since those are
actionable items.

# Code rules

**PRs are single-concern.** Each PR is one focused change that's trivial to
understand at a glance. Never bundle unrelated fixes into a grab-bag PR — if an
audit surfaces N findings, that's N PRs (or commits queued for separate PRs),
not one.

**YAGNI.** Don't leave dead code around. Delete at the root, not at the consumer
— filtering or guarding around dead code preserves it. After deletion, fields
that existed only to model the dead case become required. *(See
`principles/yagni.md`. Pairs with question-necessity: YAGNI is retrospective,
question-necessity is prospective.)*

**Question necessity before any change.** Before any fix, refactor, or addition:
ask "do we need this?" and "what could change so we don't need this?" The
duplicate might already be handled upstream; the bug might be a symptom of a
deeper problem; the new abstraction might dissolve if the right concept is
introduced. The meta-question fires before the implementation. *(See
`principles/question-necessity.md`.)*

**Compose existing primitives before adding new ones.** When a need arises, list
the primitives already in the system that touch the concern and sketch how they
combine into the answer. Adding a new primitive — a function, method, keyword,
endpoint, trait, config flag — when existing ones combine is a major red flag. A
domain name in the new primitive's name (a specific provider, ontology, vendor,
external service) is a tell that you're baking a specific case into vocabulary
that should stay generic. *(See `principles/composable-primitives.md`. Pairs
with question-necessity and YAGNI.)*

**Write today's shape, not tomorrow's.** Code is cheap to write and cheap to
rewrite. Don't introduce today-wrong shapes to make tomorrow's change smaller.
If today's logic always returns 0 or 1 items, the type is `Option<T>` — not
`Vec<T>` because a future change might return 2. If today's branch never fires,
delete it. If a param never varies across callers, drop it. If a field no
consumer reads, remove it. Pre-shaping for tomorrow introduces today's
anti-patterns — empty-Vec sentinels masking absence, always-default params, dead
branches, unused fields, helpers that wrap absent functionality — and each one
is its own code smell. The downstream change widens `Option` to `Vec`, adds the
param back, regrows the branch; that cost is small (search-and-replace) and the
anti-pattern eliminates itself when the real shape arrives. The opposite cost —
carrying anti-patterns through every reader, test, and review — recurs forever.
The chain-PR rule "Don't pre-scaffold for downstream chain PRs"
(instructions-agent.md) is a specific application of this principle. *(Pairs
with YAGNI and question-necessity: question-necessity is prospective at the
moment of design, this rule fires when designing for a future you don't yet
have, YAGNI is retrospective on dead code that's already there.)*

**Don't duplicate existing logic.** Before adding code, check whether the same
logic already lives elsewhere in the repo. A push-event handler that re-derives
state already computed by a reducer should call into the existing
implementation, or factor the shared piece into a helper both call — not
parallel it. Two implementations of the same thing drift; the one updated first
becomes a silent bug in the other.

**Don't create duplicate types.** Don't create a `FooInfo` variant of `Foo` for
display — use the full type and ignore the extra fields. Only a violation when
the original type is *usable at the new type's location*. Mirrors forced by a
boundary the original can't cross are not violations — FFI/codegen (uniffi
bridge types), serialization/wire DTOs, public-API-stability shims,
cross-language interop. Test: delete the new type and use the original; if a
boundary forbids that, it's a mandated mirror, not a duplicate.

**Parameterize near-duplicates when cheap.** Two or more near-copy blocks
introduced in the same change should collapse into one function/component with
parameters when the parameterization is small, local, and the variation maps to
a real distinction (a literal, a different type, a small expression). Skip when
the abstraction would be awkward, invent a parameter without meaning, or couple
unrelated callers — but the default for obvious near-copies is to parameterize.

**Dependency injection.** Initialize dependencies at the top and pass them down.
No singletons.

**Never mask errors with defaults.**

> Blocking

No `unwrap_or_else(|| "default")`, no `?? <default>`, no `try?` when the error
matters, no `.ok()` dropping a Result, no `let _ = …` swallowing one, and no
silent control-flow skip — `continue`, `break`, early `return`, `return None`,
or `let Some(x) = … else { continue; };` style guards — when the guarded
condition is exceptional on the main path. Covers every language idiom that
swallows or defaults around an error: Rust `unwrap_or` / `.ok()` / `let _ = …`,
Swift `try?` / `??`, and equivalents. No carve-outs for "display defaults" or
"semantically correct nil" — empty IS the value in question, and silencing it is
the masking. If data should be present, its absence is a bug — surface it with
`expect()`, `Result`, or make the types prevent it. Masking is bad on every
axis: it hides broken assumptions and creates silent failures downstream; it
impedes structure discovery by erasing the failure cases that are part of the
system's real shape (when *can* this fail? what does it mean when it does? what
should the type encode?); and it lies about correctness — code that "works"
because errors are silenced isn't working, just quiet.

The *only* escape hatch — when a silent skip is genuinely correct behavior (the
value really is optional, the case really should be skipped) — is to log the
bail-out at the skip point: `warn!` if it's rare/abnormal, `debug!` if it's
common-but-noteworthy. This applies equally to expression-level swallowers
(`unwrap_or`, `.ok()`) and control-flow skips (`continue`, `break`, early
`return`, guard-`else`). Include the input that triggered it; "skipping CUE path
with no UTF-8 stem: {:?}" is actionable, "skipping" alone is useless. This is
the only exception. If you don't want to log, you don't have a legitimate skip —
you have a masked error. (Pure functional Option-returning helpers don't count
as bail-outs; this covers exceptional skips on the main path.)

API design follows: when a function would take `Option<T>` just to `unwrap_or`
internally, split into two — one that requires the value, a `_default()` wrapper
that generates the default and calls the first. The default stays explicit at
the boundary, not buried inside.

```rust
// Bad — Option<String> exists so internal unwrap_or can fire
pub fn create_library(name: Option<String>) -> Result<Config> {
    let name = name.unwrap_or_else(generate_library_name);
    ...
}

// Good — required param, separate default wrapper
pub fn create_library(name: String) -> Result<Config> { ... }
pub fn create_library_default() -> Result<Config> {
    create_library(generate_library_name())
}
```

**Never fill in arguments with zero-valued defaults.** Sibling to the rule
above, on the construction side. When you don't know what a parameter should be,
don't pass `0`/`nil`/`None`/`""` because it "looks safe" — trace to the real
value at the source. The default that type-checks often silently breaks
downstream (a `samples_to_skip: 0` looks harmless but causes seconds of replay
artifacts in audio playback). If the parameter genuinely is optional, the
signature should use `Option`/`Optional` so the absence is explicit. Exception:
test code can pass defaults for parameters not exercised by the test.

**Many fields → `None` together means a missing type.**

> Blocking

When you see code that sets several fields of a struct to `None`/`nil` in the
same conditional block, the type is hiding a missing variant. The fields that
flip together belong in their own type — extract a sub-struct, or model the
distinction as an enum variant on a parent type. Per-field `Option` should mean
"this field is individually optional in the domain." Using per-field `Option`s
to express "this whole subset is absent" overloads the type and forces every
consumer to know the implicit discriminator.

```rust
// Bad — the "every pressing field nilled" block reveals a missing type.
let mut edit = ReleaseUserEdit {
    year: detail.year,
    format: detail.format,
    label: detail.label,
    country: detail.country,
    barcode: detail.barcode,
    ..
};
if matches!(choice, Approximate | Unknown) {
    edit.year = None;
    edit.format = None;
    edit.label = None;
    edit.country = None;
    edit.barcode = None;
}

// Good — name the cluster.
struct PressingEdit { year, format, label, country, barcode, .. }
impl PressingEdit { fn blank() -> Self { ... } }

let pressing = match choice {
    Exact { .. } => PressingEdit { year: detail.year, ... },
    Approximate { .. } | Unknown => PressingEdit::blank(),
};
let edit = ReleaseUserEdit { album_title, pressing, tracks };
```

Naming the discriminator structurally (sub-struct, enum variant) makes the
absence visible at the type level instead of buried in per-consumer
conditionals.

**Use the project's logger for real logs.** Any log that will live in committed
code goes through the project's structured logger —
`tracing::info!`/`warn!`/`error!`, the project's `Logger.<category>` helper, the
language's standard logging crate, whatever the codebase uses. Not
`println!`/`print`/`console.log`. Structured loggers give you levels,
categories, filtering, persistence; stdout prints don't. Temporary investigative
prints are fine during active debugging but must be removed before commit.

**Every bug fix starts with a failing test.** *Before* you debug, before you
even investigate — write a test that reproduces the failure. Run it, confirm it
fails. Then fix the code, run again, confirm it passes. No exceptions — even for
"obvious" fixes. The failing test is the receipt that you understood the bug,
not just patched a symptom; the passing test is the receipt that the fix
actually addressed it. When narrating a bug fix, don't say "The fix: …" before
there's a test — say "The test: …" first.

**Test the real unit, not a reconstruction.** The test must call the actual
function or service that has the bug. Manually reconstructing the conditions in
isolation (calling sub-functions in the order you think causes the bug) is just
another program — it proves nothing about the real code. Identify the unit that
contains the bug, write a test that exercises that unit.

**Never re-implement production logic in tests, mocks, or previews.** Tests
should exercise the real production code with mock *inputs*, not re-derive its
outputs. If your test setup duplicates what production does, you're checking the
duplicate against itself, not validating production. Anti-patterns:

- A `FooTestImpl` that hand-codes the same business logic `Foo` has.
- A test helper that constructs the expected output by running its own version
  of the algorithm.
- Mocking so deep that the test exercises the mock chain, not the production
  module.
- A SwiftUI `#Preview` that rewrites the view's body to make it render — now the
  preview shows a parallel implementation, not what ships.

**No transient references in code, tests, or notes.** Don't reference the
current task, fix, plan, or session in tests, comments, docstrings, or design
notes. This includes, equally:

- **Temporal phrases** — "repro for today's bug", "fails on current code", "the
  Downloads issue", "for now", "until X lands".
- **Plan / task / step codes** — `A7`, `A9`, `B3`, `V3`, `plan 01`, `plan X1`,
  `phase 2`, `step 3`. Any letter/number tag from a planning doc. These are the
  most-missed kind: a doc comment like
  `/// Apply a user-supplied metadata edit (A7 — EditMetadataSheet) to a release`
  is a violation — `A7` is a plan step, and `(A7 — EditMetadataSheet)` will rot
  once the plan ships and the caller is renamed.
- **"will replace / will substitute / lands later" forward references** to
  unshipped work — `A2 will replace this`, `the next plan reintroduces`.

Flag every added line carrying any of these. Describe the timeless invariant
instead. Transient context belongs in commit messages and PR descriptions, not
in committed code.

**Documentation describes current state; transitions go in `plans/`.** `notes/`
is the timeless "what is" — current code, current architecture, current data
shapes; no transient references, no aspirations. `plans/` is the "how we're
getting/got there" — migrations, design proposals, transition specs. Stale
references in `notes/` (renamed/deleted/refactored) are deletions, not
placeholders. Track `notes/` in version control; `plans/` is typically
gitignored (local working state).

**Look up the latest version when adding a new dependency.** Don't guess from
memory or copy from elsewhere in the codebase. Check the registry (crates.io,
npm, etc.) for the current version, then pin to that.

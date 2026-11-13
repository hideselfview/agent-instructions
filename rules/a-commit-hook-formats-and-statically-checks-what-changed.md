---
digest: A pre-commit hook does two things only, formatting and static analysis, only on what the commit changed, and finishes within 45 seconds; tests of any kind, including the tools' own tests, never run in a hook.
paths:
  - '**/hooks/**'
  - '**/*.sh'
  - '**/*.py'
  - '**/*.toml'
  - '**/*.yml'
blocking: true
---

**A commit hook formats and statically checks what changed, nothing else.**

What a pre-commit hook may run, and all it may run:

- **Formatting:** the formatters of the staged files' languages (`cargo fmt`,
  swift-format, ktlint, dotnet format), on those files or their crate.
- **Static analysis:** checks that read the source without running it: lints
  (clippy, swiftlint, detekt, actionlint), compile checks, and the repo's own
  structural checkers (bae's code-ownership checker and boundary checks).

What it never runs:

- **Tests of any kind:** unit, integration, screenshot, and the tests of the
  repo's own tools and checkers (a checker's `-test.py`, the hook's own tests).
  A tool's tests run where the tool is developed, or in CI, never on every
  commit of the repo that uses it.
- **Checks of the developer's machine:** a local dependency's copy, build
  folders, toolchain state.

How it runs:

- **Only on what changed.** Each step is gated on the staged paths it reads, and
  reads only those files, or the crates they belong to. A commit of boards, docs
  or spec runs no Rust, Swift or Kotlin step.
- **Within 45 seconds** on a warm build. A step that can't fit goes to CI, or is
  cut down to the changed files.

Before adding a step, ask: is it formatting or static analysis, does it read
only what the commit changed, and does the hook still finish in 45 seconds? If
any answer is no, it doesn't go in the hook.

Example (bae, 2026-11-13): a commit of boards and design-system text sat over
five minutes in the hook. It ran the hook's own tests (one twice), seven tests
of bae's Python checkers, `cargo fmt` over every crate, and repo-wide scans no
staged file touched; one checker's test took 3.5 minutes. The person: "hooks
should run ONLY ON THINGS THAT CHANGED, should not take longer than 45 seconds",
and what belongs in them is "static analysis, formatting. THAT's IT".

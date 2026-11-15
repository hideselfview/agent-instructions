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

## A commit hook formats and statically checks what changed, nothing else

A pre-commit hook runs these, and only these:

- **Formatters** of the staged files' languages (`cargo fmt`, swift-format,
  ktlint, dotnet format), on those files or their crate.
- **Static analysis**, checks that read the source without running it:
  - lints (clippy, swiftlint, detekt, actionlint);
  - compile checks;
  - the repo's own structural checkers, such as bae's code-ownership checker and
    boundary checks.

A hook never runs:

- **Tests of any kind:** unit, integration, screenshot, and the tests of the
  repo's own tools and checkers (a checker's `-test.py`, the hook's own tests).
  - A tool's tests run where the tool is developed, or in CI.
- **Checks of the developer's machine:** a local dependency's copy, build
  folders, toolchain state.

Each step runs only on what changed and fits the time limit:

- **Only on what changed.** Each step depends on the staged paths it reads, and
  reads only those files or the crates they belong to.
  - A commit of boards, docs or spec runs no Rust, Swift or Kotlin step.
- **Within 45 seconds** on a warm build.
  - A step that can't fit moves to CI, or is cut down to the changed files.

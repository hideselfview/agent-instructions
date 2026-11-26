# builds

builds is the Rust tool in agent-instructions (scripts/builds) that manages
worktrees, builds, reviews, landings and the work lists for several projects.

## Hard limits

- Never edit, build or commit in `~/dev/agent-instructions`, whose main checkout
  is the live copy (builds help, its opening).
- Run every cargo command through `builds cargo` (builds help workflow, BUILDS).
- Try a change with the worktree's own `scripts/builds/target/debug/builds`,
  never the installed builds (builds help, its opening).

## Dashboard shots

A change to the dashboard (scripts/builds/src/daemon) takes its shots in its app
build, with
`builds app-build agent-instructions <branch> --shots web.dashboard.` or a
longer prefix of the screens the change touches. Each shot stages the fixture of
its name in scripts/builds/src/daemon/shots and compares the page with its board
in design/mocks.

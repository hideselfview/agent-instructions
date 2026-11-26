# builds

builds is the Rust tool in agent-instructions (scripts/builds) that manages
worktrees, builds, reviews, landings and the work lists for several projects.

## Hard limits

- Never edit, build or commit in `~/dev/agent-instructions`, whose main checkout
  is the live copy (builds help, its opening).
- Run every cargo command through `builds cargo` (builds help workflow, BUILDS).
- Try a change with the worktree's own `scripts/builds/target/debug/builds`,
  never the installed builds (builds help, its opening).

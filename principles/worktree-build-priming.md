# Agent worktrees with build-priming hooks (Rust + Swift/Xcode)

Worktree mechanics for projects whose `post-checkout` hook primes a heavy build
on `git worktree add` — a SwiftPM/xcodebuild prime, a dependency compile,
codegen. Pairs with the worktree rules in `instructions-agent.md` ("Worktrees
are for agent parallelism"); this file is the build-infra layer those rules
gesture at with "whatever per-worktree setup the project runs."

## The prime hook is slow and, under sccache, concurrency-hostile

A `post-checkout` hook that compiles on worktree creation has two costs:

- **Slow.** Every `git worktree add` pays a full prime.
- **Races sccache.** When the prime compiles through `sccache`
  (`RUSTC_WRAPPER=sccache`, common in the repo's `.cargo/config.toml` or the
  shell env), two worktree creations running at once race sccache's shared temp
  dir and fail: `error: couldn't create a temp dir: No such file or directory`.
  Spawning N agents that each create an isolated worktree triggers exactly this.

So never create sccache-backed worktrees concurrently. Serialize creation, or
skip the prime (below) so there is no concurrent compile to race.

## Skip the prime when the work doesn't need it

A prime that warms a *platform* build (macOS/iOS/Android) is dead work for a
change confined to a lower layer (the Rust core) — provided the pre-commit hook
already skips that platform build when no platform files changed (a good
pre-commit hook does; confirm it for the project). Create the worktree without
the setup hook, then do by hand only what the core build actually needs:

```sh
git -c core.hooksPath=/dev/null worktree add -b <branch> <path> <base-ref>
# minimal setup the core build needs, e.g.:
ln -sfn <main-checkout>/<native-libs-dir> <path>/<native-libs-dir>  # prebuilt native deps
ln -sf <project-CLAUDE.md-source> <path>/CLAUDE.md                  # project rules
```

This skips only the **setup/prime** hook. The commit-time **pre-commit
verification** hook still runs by default — skipping a build prime is not
bypassing verification. Skipping the commit hook requires the user's explicit
authorization under the main agent workflow policy.

Native-library paths a core build links against usually come from the repo's
`.cargo/config.toml` (`LIBRARY_PATH`, `PKG_CONFIG_PATH`), so a lean worktree
builds the core with no prime at all.

## A per-project target dir dissolves both sccache failures

Both failures below have one cause: sccache writes its temp files inside the
target dir's `deps/`, and a brand-new `CARGO_TARGET_DIR` has none. Per-worktree
target dirs manufacture that condition on every worktree creation — which is why
projects end up disabling sccache to work around it, losing the cache to avoid a
problem the fresh dir created, and then paying for it on every build.

Point every worktree of a project at one target dir instead. It is warm after
the first build and never fresh again, so neither failure can occur, and the
worktrees stop duplicating ~26 GB of linked artifacts each.

Derive it in `~/.zshenv`, not `~/.zshrc` — git hooks and agent-run builds are
non-interactive shells, which never source `.zshrc`, so guidance placed there
silently does nothing for exactly the builds that matter:

```sh
() {
  local common_dir project
  common_dir=$(git rev-parse --git-common-dir 2>/dev/null) || {
    export CARGO_TARGET_DIR="$HOME/.cargo-target/_shared"; return
  }
  project=${${common_dir:A:h}:t}
  export CARGO_TARGET_DIR="$HOME/.cargo-target/${project:-_shared}"
}
```

`--git-common-dir` rather than `--show-toplevel`: in a worktree the toplevel is
the worktree, while the common dir points at the main checkout, so every
worktree of a project resolves to the same name whether worktrees sit inside the
repo or beside it.

Per *project*, not one global dir: cargo locks the target directory for the
length of a build, so a single shared dir makes one project's build block
another's. Keep `RUSTC_WRAPPER=sccache` and `CARGO_INCREMENTAL=0` alongside it —
sccache cannot cache incremental compilations, and incremental is the weaker
choice across worktrees anyway, since its state is keyed by crate and profile
rather than by source, so two branches repeatedly invalidate each other.
Measured on one repo: a one-file core rebuild went from 41.6s to 13.3s.

Also drop any project instruction telling agents to pass `CARGO_TARGET_DIR=…` or
`RUSTC_WRAPPER=` inline. An inline env var overrides the shared setting, so
following that guidance re-creates the cold per-worktree dir the setup exists to
avoid.

## sccache + a fresh target dir: build once, then reuse it

sccache also fails to write into a brand-new `CARGO_TARGET_DIR` — the `deps/`
subdir doesn't exist yet — the same `couldn't create a temp dir` error, with no
concurrency involved. Build once into a target dir to establish it, then reuse
that one dir for every later build, test, and commit in the worktree.

The pre-commit hook compiles too, so pass the same env inline on the commit or
its build hits a fresh dir and fails:

```sh
CARGO_TARGET_DIR=<dir> RUSTC_WRAPPER= git commit -m "..."
```

`RUSTC_WRAPPER=` (empty) disables sccache for that build, sidestepping the
temp-dir race entirely; reuse one warm target dir so rebuilds stay incremental
despite the lost cache.

## SPM / Sparkle cache recovery (Swift/Xcode + SwiftPM)

If xcodebuild (in the prime or the pre-commit hook) fails with "Couldn't check
out revision" or "file not found" on a SwiftPM dependency (Sparkle is a frequent
offender), its checkout cache is corrupt. Clear it and rebuild:

```sh
rm -rf <app>/.build/derivedData
rm -rf ~/Library/Caches/org.swift.swiftpm/repositories/<Dep>*
cd <app> && xcodebuild -project <proj>.xcodeproj -scheme <scheme> \
  -derivedDataPath .build/derivedData build
```

VPN can cause incomplete git fetches that corrupt the SwiftPM cache; disconnect
and retry. The prime hook must use the **same** xcodebuild flags as the
pre-commit hook (notably `-derivedDataPath`) — otherwise the primed packages
land where the pre-commit xcodebuild can't find them and it fails.

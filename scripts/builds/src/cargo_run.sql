-- The record of every `builds cargo` run, in `builds.sqlite` in a project's
-- store folder. cargo_run.rs owns this table and is the only code that writes
-- it. Each row is what one run observed when cargo exited: when it started,
-- cargo's subcommand, the host cargo ran on, that host's cores and load when
-- builds picked it, how long cargo waited for its file locks, how long the
-- run took, and the hook check that started it, when one did. hook_stats.rs
-- reads these rows, through cargo_run.rs, to show where each hook check that
-- ran `builds cargo` ran, and `builds cargo` reads the host of a checkout's
-- last run of a subcommand to send the next run of it there (cargo_place.rs).

CREATE TABLE builds_cargo_runs (
    id INTEGER PRIMARY KEY,
    -- The top folder of the checkout whose files cargo built.
    worktree TEXT NOT NULL CHECK (worktree <> ''),
    -- When `builds cargo` started, seconds since 1970. A run that builds
    -- recorded before it kept this column holds the start of the hook run
    -- whose check started it: the hook planned its checks and ran its fixes
    -- between the two.
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    -- cargo's subcommand: `test`, `clippy`, or `nextest` and nextest's own,
    -- as in `nextest run`. NULL for a run that builds recorded before it kept
    -- this column; a hook check started each of those.
    subcommand TEXT CHECK (subcommand <> ''),
    -- `local` for this Mac, else the host's name.
    host TEXT NOT NULL CHECK (host <> ''),
    -- The host's cores and its one-minute load average when builds picked it.
    cores INTEGER NOT NULL CHECK (cores > 0),
    load REAL NOT NULL CHECK (load >= 0),
    -- How long cargo waited for its file locks; NULL when cargo printed to a
    -- terminal, which builds leaves to cargo and does not read.
    lock_wait_ms INTEGER CHECK (lock_wait_ms >= 0),
    -- From the start to cargo's exit.
    ms INTEGER NOT NULL CHECK (ms >= 0),
    -- The hook check that started the run: the start of the hook's run (its
    -- `started_at`) in the same worktree, and the check's position in it.
    -- Both columns are NULL for a run that no hook check started.
    hook_started_at INTEGER CHECK (hook_started_at > 0),
    hook_position INTEGER CHECK (hook_position >= 0),
    CHECK ((hook_started_at IS NULL) = (hook_position IS NULL)),
    CHECK (subcommand IS NOT NULL OR hook_started_at IS NOT NULL),
    CHECK (lock_wait_ms <= ms)
) STRICT;

CREATE INDEX idx_builds_cargo_runs_hook ON builds_cargo_runs (worktree, hook_started_at);
CREATE INDEX idx_builds_cargo_runs_started ON builds_cargo_runs (started_at);

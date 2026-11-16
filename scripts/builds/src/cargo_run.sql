-- The record of `builds cargo` runs, in `builds.sqlite` in a project's store
-- folder. cargo_run.rs owns this table and is the only code that writes it.
-- Each row is what one run observed when cargo exited: the host cargo ran on,
-- that host's cores and load when builds picked it, how long cargo waited for
-- its file locks and how long the run took. The hook's figures read it back
-- for the checks that ran builds cargo (hook_stats.rs).

CREATE TABLE builds_cargo_runs (
    id INTEGER PRIMARY KEY,
    -- When the run started, seconds since 1970.
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    repo TEXT NOT NULL CHECK (repo <> ''),
    -- The top folder of the checkout whose files cargo built.
    worktree TEXT NOT NULL CHECK (worktree <> ''),
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
    -- `started_at`) in the same worktree, and the check's position in it. Both
    -- NULL for a run that no hook check started.
    hook_started_at INTEGER CHECK (hook_started_at > 0),
    hook_position INTEGER CHECK (hook_position >= 0),
    CHECK (lock_wait_ms <= ms),
    CHECK ((hook_started_at IS NULL) = (hook_position IS NULL))
) STRICT;

CREATE INDEX idx_builds_cargo_runs_hook ON builds_cargo_runs (worktree, hook_started_at);

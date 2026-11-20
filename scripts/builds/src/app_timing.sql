-- The record of app-build runs, in `builds.sqlite` in a project's store
-- folder. app_timing.rs owns these tables and is the only code that writes
-- them. Nothing here is derived: when a run waited and built, and how long
-- each step took, are facts a run observed; medians and slowest times are
-- queries over them.

-- One row per lane's run of `builds app-build`, written when the run ends,
-- whether it passed or not. A run that never took a checkout is not written.
CREATE TABLE IF NOT EXISTS app_build_runs (
    id INTEGER PRIMARY KEY,
    -- When the run joined the lane's queue, seconds since 1970.
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    repo TEXT NOT NULL CHECK (repo <> ''),
    lane TEXT NOT NULL CHECK (lane IN ('mac', 'android')),
    -- The app checkout (side-N) the run built in.
    checkout TEXT NOT NULL CHECK (checkout <> ''),
    -- The ref the caller named.
    built_ref TEXT NOT NULL CHECK (built_ref <> ''),
    -- The claim of the worktree the caller ran in; NULL when it has none.
    who TEXT CHECK (who <> ''),
    -- How long the run waited in the lane's queue before it took a
    -- checkout, a wait after a takeover included.
    wait_ms INTEGER NOT NULL CHECK (wait_ms >= 0),
    -- How the run ended. `stopped`: a stop signal (SIGTERM, SIGINT or
    -- SIGHUP) reached it before it passed.
    outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed', 'stopped')),
    -- The cores and the one-minute load average of the host of the checkout,
    -- when the run took the checkout. Both columns are NULL for a run that
    -- builds recorded before it kept them, and for a run whose host did not
    -- answer when builds asked it for its load.
    cores INTEGER CHECK (cores > 0),
    load REAL CHECK (load >= 0),
    CHECK ((cores IS NULL) = (load IS NULL))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_app_build_runs_started ON app_build_runs (started_at);

-- One row per step the run started, in the order started: a step of
-- `.app-build`, or a `--test` class or `--shots` selection. A step that a
-- takeover stopped has a row too, and the run's next attempt another. A step
-- that a stop signal ended is `stopped`.
CREATE TABLE IF NOT EXISTS app_build_steps (
    run_id INTEGER NOT NULL REFERENCES app_build_runs (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    name TEXT NOT NULL CHECK (name <> ''),
    kind TEXT NOT NULL CHECK (kind IN ('step', 'test', 'shots')),
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    wall_ms INTEGER NOT NULL CHECK (wall_ms >= 0),
    -- User and system CPU time of the step's processes.
    cpu_ms INTEGER NOT NULL CHECK (cpu_ms >= 0),
    result TEXT NOT NULL CHECK (result IN ('passed', 'failed', 'timed out', 'taken over', 'stopped')),
    PRIMARY KEY (run_id, position)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_app_build_steps_name ON app_build_steps (name);

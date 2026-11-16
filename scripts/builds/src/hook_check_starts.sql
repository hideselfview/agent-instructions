-- This table records this Mac's core count and one-minute load average when
-- each check of a hook run started; it lives in `builds.sqlite` in a
-- project's store folder. hook_check_starts.rs owns this table and is the
-- only code that writes it, in the transaction that writes the run
-- (hook_stats.rs). For a check that ran `builds cargo`, `builds_cargo_runs`
-- (cargo_run.sql) also holds the host cargo ran on, with that host's load,
-- keyed by the hook run's worktree, its start time and the check's position.

-- One row per check that ran (passed or failed); a skipped check has none,
-- and neither has a check of a run recorded before this table.
CREATE TABLE hook_check_starts (
    run_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    cores INTEGER NOT NULL CHECK (cores > 0),
    load REAL NOT NULL CHECK (load >= 0),
    PRIMARY KEY (run_id, position),
    FOREIGN KEY (run_id, position) REFERENCES hook_run_checks (run_id, position) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

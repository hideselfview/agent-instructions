-- The record of pre-commit hook runs, in `builds.sqlite` in a project's store
-- folder. hook_stats.rs owns these tables and is the only code that writes
-- them. Nothing here is derived: a run's pass or fail beyond its checks, its
-- timings and its staged files are facts a hook run observed; counts, medians
-- and extensions are queries over them.

-- One row per `builds checks run`.
CREATE TABLE IF NOT EXISTS hook_runs (
    id INTEGER PRIMARY KEY,
    -- When the run started, seconds since 1970.
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    repo TEXT NOT NULL CHECK (repo <> ''),
    worktree TEXT NOT NULL CHECK (worktree <> ''),
    -- NULL for a detached HEAD.
    branch TEXT CHECK (branch <> ''),
    -- The run's exit status: not derivable from its checks, since a run also
    -- fails for what no check reports (an unreadable `.checks`, a staging
    -- that git refused).
    passed INTEGER NOT NULL CHECK (passed IN (0, 1)),
    -- Reading `.checks`, listing the staged files and picking the lines
    -- (`cargo metadata` for `{crates}` included).
    plan_ms INTEGER NOT NULL CHECK (plan_ms >= 0),
    -- From the start to the last check's end.
    total_ms INTEGER NOT NULL CHECK (total_ms >= plan_ms)
, base TEXT CHECK (base <> '')) STRICT;

CREATE INDEX IF NOT EXISTS idx_hook_runs_started ON hook_runs (started_at);

-- One row per file of the run's staged diff.
CREATE TABLE IF NOT EXISTS hook_run_files (
    run_id INTEGER NOT NULL REFERENCES hook_runs (id) ON DELETE CASCADE,
    path TEXT NOT NULL CHECK (path <> ''),
    change TEXT NOT NULL CHECK (change IN ('added', 'modified', 'deleted')),
    -- Lines added and removed; both NULL for a binary file.
    added INTEGER CHECK (added >= 0),
    removed INTEGER CHECK (removed >= 0),
    PRIMARY KEY (run_id, path),
    CHECK ((added IS NULL) = (removed IS NULL))
) STRICT, WITHOUT ROWID;

-- One row per check or fix of the repo's `.checks`, in file order: the ones
-- that ran and the ones the staged files did not pick.
CREATE TABLE IF NOT EXISTS hook_run_checks (
    run_id INTEGER NOT NULL REFERENCES hook_runs (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    name TEXT NOT NULL CHECK (name <> ''),
    kind TEXT NOT NULL CHECK (kind IN ('check', 'fix')),
    outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed', 'skipped')),
    -- What it was handed, and how long it took: NULL for a skipped one.
    files INTEGER CHECK (files >= 0),
    crates INTEGER CHECK (crates >= 0),
    ms INTEGER CHECK (ms >= 0),
    PRIMARY KEY (run_id, position),
    UNIQUE (run_id, name),
    CHECK ((outcome = 'skipped') = (files IS NULL)),
    CHECK ((outcome = 'skipped') = (crates IS NULL)),
    CHECK ((outcome = 'skipped') = (ms IS NULL))
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_hook_run_checks_name ON hook_run_checks (name, kind);

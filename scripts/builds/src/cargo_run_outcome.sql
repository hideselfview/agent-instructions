-- How each `builds cargo` run ended. cargo_run_outcome.rs owns these tables
-- and is the only code that writes them; cargo_run.rs writes them in the
-- transaction that writes the run. A run that builds recorded before it kept
-- outcomes has no row.

CREATE TABLE builds_cargo_run_outcome (
    run_id INTEGER PRIMARY KEY REFERENCES builds_cargo_runs (id) ON DELETE CASCADE,
    -- `passed`: cargo exited 0. `failed`: cargo exited with another status.
    -- `stopped`: a signal ended cargo, or builds stopped it.
    outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed', 'stopped')),
    -- For a run that did not pass, the lines of cargo's output that say why:
    -- from the first test panic or compiler error, else its last lines. NULL
    -- for a run that passed, or one whose output builds did not read (cargo
    -- printed to a terminal).
    excerpt TEXT CHECK (excerpt <> '')
) STRICT;

-- The tests a run reported failed, by the names cargo test or nextest printed.
CREATE TABLE builds_cargo_run_failed_test (
    run_id INTEGER NOT NULL REFERENCES builds_cargo_runs (id) ON DELETE CASCADE,
    test TEXT NOT NULL CHECK (test <> ''),
    PRIMARY KEY (run_id, test)
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_builds_cargo_run_failed_test ON builds_cargo_run_failed_test (test);

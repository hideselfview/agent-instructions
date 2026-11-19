-- What a failed check of a hook run said. hook_check_failure.rs owns these
-- tables and is the only code that writes them, in the transaction that
-- writes the run (hook_stats.rs). A check that passed or was skipped has no
-- rows, and neither has a check of a run that builds recorded before it kept
-- them.

-- The lines of the check's output that say why it failed
-- (output_excerpt.rs); a failed check that printed nothing has no row.
CREATE TABLE hook_check_excerpt (
    run_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    excerpt TEXT NOT NULL CHECK (excerpt <> ''),
    PRIMARY KEY (run_id, position),
    FOREIGN KEY (run_id, position) REFERENCES hook_run_checks (run_id, position) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- Each staged file the check was handed that its output names: the files it
-- failed on.
CREATE TABLE hook_check_failed_file (
    run_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    path TEXT NOT NULL CHECK (path <> ''),
    PRIMARY KEY (run_id, position, path),
    FOREIGN KEY (run_id, position) REFERENCES hook_run_checks (run_id, position) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

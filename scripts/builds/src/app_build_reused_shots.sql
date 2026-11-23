-- The shots comparisons an app build reused for its commit.
-- app_build_reused_shots.rs writes them with the run and its steps.

CREATE TABLE app_build_reused_shots (
    run_id INTEGER NOT NULL REFERENCES app_build_run_commit (run_id) ON DELETE CASCADE,
    prefix TEXT NOT NULL CHECK (prefix <> ''),
    PRIMARY KEY (run_id, prefix)
) STRICT, WITHOUT ROWID;

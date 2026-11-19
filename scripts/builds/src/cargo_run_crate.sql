-- The crates a `builds cargo` run named with `-p` or `--package`. cargo_run_crate.rs
-- owns this table and is the only code that writes it. A run that names none
-- builds the workspace's default members and has no row here.

CREATE TABLE builds_cargo_run_crate (
    run_id INTEGER NOT NULL REFERENCES builds_cargo_runs (id) ON DELETE CASCADE,
    crate TEXT NOT NULL CHECK (crate <> ''),
    PRIMARY KEY (run_id, crate)
) STRICT, WITHOUT ROWID;

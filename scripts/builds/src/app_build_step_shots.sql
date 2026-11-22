-- What each shots step of an app build said of the two caches it reads.
-- app_build_step_shots.rs owns this table and is the only code that writes
-- it, in the transaction that writes the run (app_timing.rs). A step that
-- is no shots step, and a shots step of a run that builds recorded before it
-- kept these facts, has no row.

-- One row per shots step. `environment` is the host software that drew the
-- step's shots (shots_environment.rs). `base_run` says whether the run of the
-- merge base that the step compares with was already saved when the build
-- looked for it (`saved`), had to be built in the build's turn (`built`), or
-- was not needed: the commit is on origin/main, or the step's prefix starts
-- no shot declared at the merge base. `reused` and `captured` count the
-- screenshots the step took from the picture folder and the ones it shot;
-- both are NULL for a step that ended before its run finished.
CREATE TABLE app_build_step_shots (
    run_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    environment TEXT NOT NULL CONSTRAINT app_build_step_shots_environment_says_something CHECK (environment <> ''),
    base_run TEXT NOT NULL CONSTRAINT app_build_step_shots_base_run_is_known CHECK (base_run IN ('not needed', 'saved', 'built')),
    reused INTEGER CONSTRAINT app_build_step_shots_reused_counts CHECK (reused >= 0),
    captured INTEGER CONSTRAINT app_build_step_shots_captured_counts CHECK (captured >= 0),
    CONSTRAINT app_build_step_shots_counts_come_together CHECK ((reused IS NULL) = (captured IS NULL)),
    PRIMARY KEY (run_id, position),
    FOREIGN KEY (run_id, position) REFERENCES app_build_steps (run_id, position) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

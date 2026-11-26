-- Why the merge base's saved run was missing when a shots step looked for it.
-- app_build_base_misses.rs owns this table and is the only code that writes
-- it, in the transaction that writes the step's row there. A step whose
-- base run was saved or not needed has no row, and neither has a built step
-- of a run that builds recorded before it kept the cause.

-- One row per shots step whose `base_run` is `built`. `cause` names what
-- the cache held for the merge base's commit and lane when the build looked:
-- `program`: a run of a covering prefix in the step's environment, made by
-- another version of the shots program; `environment`: a run of a covering
-- prefix, made in another environment; `board`: runs of other prefixes only;
-- `none`: no run at all.
CREATE TABLE app_build_base_misses (
    run_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    cause TEXT NOT NULL CONSTRAINT app_build_base_misses_cause_is_known CHECK (cause IN ('program', 'environment', 'board', 'none')),
    PRIMARY KEY (run_id, position),
    FOREIGN KEY (run_id, position) REFERENCES app_build_step_shots (run_id, position) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

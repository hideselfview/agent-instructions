-- What a failed step of an app build said. app_build_step_failure.rs owns
-- these tables and is the only code that writes them, in the transaction that
-- writes the run (app_timing.rs). A step that passed or that another build
-- took over has no rows, and neither has a step of a run that builds recorded
-- before it kept them.

-- The lines of the output of a step that failed or ran past its limit that
-- say why (output_excerpt.rs); a step that printed nothing has no row.
CREATE TABLE app_build_step_excerpt (
    run_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    excerpt TEXT NOT NULL CHECK (excerpt <> ''),
    PRIMARY KEY (run_id, position),
    FOREIGN KEY (run_id, position) REFERENCES app_build_steps (run_id, position) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- Each shot that a failed step's shots program said fails the run, by its
-- name (`mac.welcome.first`).
CREATE TABLE app_build_step_failed_shot (
    run_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    shot TEXT NOT NULL CHECK (shot <> ''),
    PRIMARY KEY (run_id, position, shot),
    FOREIGN KEY (run_id, position) REFERENCES app_build_steps (run_id, position) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_app_build_step_failed_shot ON app_build_step_failed_shot (shot);

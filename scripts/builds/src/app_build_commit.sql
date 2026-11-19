-- The commit each app build run built. app_build_commit.rs owns this table
-- and is the only code that writes it; app_timing.rs writes it in the
-- transaction that writes the run. A run that builds recorded before it kept
-- commits has no row.

CREATE TABLE app_build_run_commit (
    run_id INTEGER PRIMARY KEY REFERENCES app_build_runs (id) ON DELETE CASCADE,
    -- The app checkout's HEAD once the run checked out the ref it built.
    built_commit TEXT NOT NULL CHECK (length(built_commit) = 40 AND built_commit NOT GLOB '*[^0-9a-f]*')
) STRICT;

-- The resolved target each app build run was asked to build. The run can
-- fail on its merge base before it checks out this commit. Old runs have
-- no row because builds did not record their targets.
CREATE TABLE app_build_requested_commit (
    run_id INTEGER PRIMARY KEY REFERENCES app_build_runs (id) ON DELETE CASCADE,
    requested_commit TEXT NOT NULL CHECK (length(requested_commit) = 40 AND requested_commit NOT GLOB '*[^0-9a-f]*')
) STRICT;

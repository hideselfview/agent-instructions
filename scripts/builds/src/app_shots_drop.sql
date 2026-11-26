-- A shots prefix an agent dropped from the landing of its branch
-- (`builds app-build --drop-shots`; app_shots_drop.rs owns this table and is
-- the only code that writes it). Land requires the app build that covers a
-- branch to run every shots prefix the branch's own app builds ran since its
-- merge base, unless a row here drops the prefix. `why` says why the branch
-- needs no run of it, and `builds land` prints each row it honors.
CREATE TABLE app_shots_drop (
    id INTEGER PRIMARY KEY,
    repo TEXT NOT NULL REFERENCES repo (name),
    -- The ref the branch's app builds named (`app_build_runs.built_ref`).
    branch TEXT NOT NULL CONSTRAINT app_shots_drop_branch_says_something CHECK (trim(branch) <> ''),
    -- The name of the app the lane builds, in the form `app_build_runs.lane`
    -- takes.
    lane TEXT NOT NULL CONSTRAINT app_shots_drop_lane_is_a_lane CHECK (length(CAST(lane AS BLOB)) <= 32 AND lane NOT GLOB '*[^-a-z0-9]*' AND '-' || lane || '-' NOT GLOB '*--*'),
    prefix TEXT NOT NULL CONSTRAINT app_shots_drop_prefix_says_something CHECK (prefix <> ''),
    at INTEGER NOT NULL CONSTRAINT app_shots_drop_at_is_a_time CHECK (at > 0),
    by TEXT NOT NULL CONSTRAINT app_shots_drop_by_says_something CHECK (trim(by) <> ''),
    why TEXT NOT NULL CONSTRAINT app_shots_drop_why_says_something CHECK (trim(why) <> '')
) STRICT;

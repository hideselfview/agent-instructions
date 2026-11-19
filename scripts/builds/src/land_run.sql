-- The record of every `builds land` run that reached a project's lists, in
-- `builds.sqlite` in the project's store folder. land_run.rs owns these tables
-- and is the only code that writes them. Each run is written once, when land
-- ends, whether the commits landed or land refused them. Nothing here is
-- derived: when each step began and how long it took are facts the run
-- observed.

CREATE TABLE land_run (
    id INTEGER PRIMARY KEY,
    -- When `builds land` started, seconds since 1970. The first step, the wait
    -- for an earlier landing in the worktree, begins here.
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    repo TEXT NOT NULL CHECK (repo <> ''),
    worktree TEXT NOT NULL CHECK (worktree <> ''),
    -- The branch the landing started on; NULL for a detached HEAD.
    branch TEXT CHECK (branch <> ''),
    -- The claim of the worktree, which the landing's records name.
    by TEXT NOT NULL CHECK (by <> ''),
    -- From the start to the end of the run.
    ms INTEGER NOT NULL CHECK (ms >= 0),
    landed INTEGER NOT NULL CHECK (landed IN (0, 1))
) STRICT;

CREATE INDEX idx_land_run_started ON land_run (started_at);

-- The items the run finished (`--done`) or advanced (`--part`).
CREATE TABLE land_run_item (
    land_id INTEGER NOT NULL REFERENCES land_run (id) ON DELETE CASCADE,
    item_id INTEGER NOT NULL REFERENCES item (id),
    PRIMARY KEY (land_id, item_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX idx_land_run_item_item ON land_run_item (item_id);

-- The commits the run was asked to land, in the order given, each with the
-- time its author committed it.
CREATE TABLE land_run_commit (
    land_id INTEGER NOT NULL REFERENCES land_run (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    sha TEXT NOT NULL CHECK (length(sha) = 40 AND sha NOT GLOB '*[^0-9a-f]*'),
    committed_at INTEGER NOT NULL CHECK (committed_at > 0),
    PRIMARY KEY (land_id, position)
) STRICT, WITHOUT ROWID;

-- The steps the run took, in the order taken. `lock`: waiting for an earlier
-- landing in the worktree. `lists-check`: the checks of the lists and of the
-- review before anything is fetched. `fetch`, `pick` (the fast-forward or
-- cherry-pick onto origin/main), `checks` (the whole-tree checks and the
-- compile check of the result) and `push`: one of each for every attempt, so
-- a refused push shows as another round. `records`: the records after the
-- push. `finish`: releasing the worktree and fast-forwarding the main checkout.
CREATE TABLE land_step (
    land_id INTEGER NOT NULL REFERENCES land_run (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    kind TEXT NOT NULL CHECK (
        kind IN ('lock', 'lists-check', 'fetch', 'pick', 'checks', 'push', 'records', 'finish')
    ),
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    ms INTEGER NOT NULL CHECK (ms >= 0),
    PRIMARY KEY (land_id, position)
) STRICT, WITHOUT ROWID;

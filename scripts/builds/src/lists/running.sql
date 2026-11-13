-- Where an item runs: one worktree per repo the work spans. running.rs owns
-- these tables and is the only code that writes them. The agent is the
-- claim of the worktree, never stored here. `item take` gives every place of
-- an item the same brief and moment, and a handoff changes all of them, so
-- brief and moment belong to the item running.

CREATE TABLE running (
    item_id INTEGER PRIMARY KEY REFERENCES item (id),
    brief TEXT NOT NULL CONSTRAINT running_brief_says_something CHECK (trim(brief) <> ''),
    since_at INTEGER NOT NULL,
    since_by TEXT NOT NULL CONSTRAINT running_since_by_says_something CHECK (trim(since_by) <> '')
) STRICT;

CREATE TABLE running_place (
    item_id INTEGER NOT NULL REFERENCES running (item_id) ON DELETE CASCADE,
    repo TEXT NOT NULL REFERENCES repo (name),
    -- A side worktree is <parent>/<repo>-worktrees/<checkout>.
    parent TEXT NOT NULL CONSTRAINT running_place_parent_is_absolute CHECK (substr(parent, 1, 1) = '/'),
    checkout TEXT NOT NULL CONSTRAINT running_place_checkout_is_a_side_worktree CHECK (
        checkout GLOB 'side-[0-9]*' AND checkout NOT GLOB 'side-*[^0-9]*'
    ),
    worktree TEXT GENERATED ALWAYS AS (parent || '/' || repo || '-worktrees/' || checkout) STORED NOT NULL,
    branch TEXT NOT NULL CONSTRAINT running_place_branch_says_something CHECK (trim(branch) <> ''),
    PRIMARY KEY (item_id, repo)
) STRICT, WITHOUT ROWID;
CREATE INDEX running_place_worktree ON running_place (worktree);

-- Where an open item holds a worktree with no agent (`builds item park`): its
-- work waits there on its branch while the item waits in the queue, and
-- taking the item moves it to `running` in the same worktree. A parked item
-- holds one worktree for each repo its work spans. parked.rs owns these tables and is the only code that
-- writes them.

CREATE TABLE parked (
    item_id INTEGER PRIMARY KEY REFERENCES item (id),
    since_at INTEGER NOT NULL,
    since_by TEXT NOT NULL CONSTRAINT parked_since_by_says_something CHECK (trim(since_by) <> '')
) STRICT;

CREATE TABLE parked_place (
    item_id INTEGER NOT NULL REFERENCES parked (item_id) ON DELETE CASCADE,
    repo TEXT NOT NULL REFERENCES repo (name),
    -- A side worktree is <parent>/<repo>-worktrees/<checkout>.
    parent TEXT NOT NULL CONSTRAINT parked_place_parent_is_absolute CHECK (substr(parent, 1, 1) = '/'),
    checkout TEXT NOT NULL CONSTRAINT parked_place_checkout_is_a_side_worktree CHECK (
        checkout GLOB 'side-[0-9]*' AND checkout NOT GLOB 'side-*[^0-9]*'
    ),
    worktree TEXT GENERATED ALWAYS AS (parent || '/' || repo || '-worktrees/' || checkout) STORED NOT NULL,
    branch TEXT NOT NULL CONSTRAINT parked_place_branch_says_something CHECK (trim(branch) <> ''),
    PRIMARY KEY (item_id, repo)
) STRICT, WITHOUT ROWID;
CREATE INDEX parked_place_worktree ON parked_place (worktree);

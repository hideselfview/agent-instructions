-- Where an open item keeps a worktree while it waits in the queue with no
-- agent (`builds item stop` records it when the worktree holds the item's
-- unfinished work): its work waits there on its branch, and taking the item
-- moves it to `running` in the same worktree. An item keeps one worktree for
-- each repo its work spans. kept_work.rs owns these tables and is the only
-- code that writes them.

CREATE TABLE kept_work (
    item_id INTEGER PRIMARY KEY REFERENCES item (id),
    since_at INTEGER NOT NULL,
    since_by TEXT NOT NULL CONSTRAINT kept_work_since_by_says_something CHECK (trim(since_by) <> '')
) STRICT;

CREATE TABLE kept_work_place (
    item_id INTEGER NOT NULL REFERENCES kept_work (item_id) ON DELETE CASCADE,
    repo TEXT NOT NULL REFERENCES repo (name),
    -- A side worktree is <parent>/<repo>-worktrees/<checkout>.
    parent TEXT NOT NULL CONSTRAINT kept_work_place_parent_is_absolute CHECK (substr(parent, 1, 1) = '/'),
    checkout TEXT NOT NULL CONSTRAINT kept_work_place_checkout_is_a_side_worktree CHECK (
        checkout GLOB 'side-[0-9]*' AND checkout NOT GLOB 'side-*[^0-9]*'
    ),
    worktree TEXT GENERATED ALWAYS AS (parent || '/' || repo || '-worktrees/' || checkout) STORED NOT NULL,
    branch TEXT NOT NULL CONSTRAINT kept_work_place_branch_says_something CHECK (trim(branch) <> ''),
    PRIMARY KEY (item_id, repo)
) STRICT, WITHOUT ROWID;
CREATE INDEX kept_work_place_worktree ON kept_work_place (worktree);

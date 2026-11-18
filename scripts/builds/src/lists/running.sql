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

-- Each run of an item that ended: it ran from `item take` until someone
-- stopped it (`item stop`) or closed it (a landing,
-- `item finished` or `item drop`, which the item's closing names). The run
-- that goes on is the item's `running` row.
CREATE TABLE item_run (
    id INTEGER PRIMARY KEY,
    item_id INTEGER NOT NULL REFERENCES item (id),
    since_at INTEGER NOT NULL,
    since_by TEXT NOT NULL CONSTRAINT item_run_since_by_says_something CHECK (trim(since_by) <> ''),
    how TEXT NOT NULL CONSTRAINT item_run_how_known CHECK (how IN ('stopped', 'closed')),
    ended_at INTEGER NOT NULL,
    ended_by TEXT NOT NULL CONSTRAINT item_run_ended_by_says_something CHECK (trim(ended_by) <> ''),
    CONSTRAINT item_run_ends_after_it_starts CHECK (ended_at >= since_at)
) STRICT;
CREATE INDEX item_run_item ON item_run (item_id);

-- The worktree and branch an ended run ran in, one per repo its work spanned.
-- A run that a store move read from the write log has none, since the log
-- names no worktree or branch.
CREATE TABLE item_run_place (
    run_id INTEGER NOT NULL REFERENCES item_run (id),
    repo TEXT NOT NULL REFERENCES repo (name),
    worktree TEXT NOT NULL CONSTRAINT item_run_place_worktree_is_absolute CHECK (substr(worktree, 1, 1) = '/'),
    branch TEXT NOT NULL CONSTRAINT item_run_place_branch_says_something CHECK (trim(branch) <> ''),
    PRIMARY KEY (run_id, repo)
) STRICT, WITHOUT ROWID;

-- The spans of the run that goes on, each from when it began: `held` while
-- the worktree's claim note says held (`builds note --state held`, `item
-- take --held`), and `waiting` while the item waits on a question or on a design
-- change the person has not approved. When a write starts an item's wait
-- on the person, builds opens the item's `waiting` span, and when a write
-- ends that wait, builds ends the span.
CREATE TABLE running_span (
    item_id INTEGER NOT NULL REFERENCES running (item_id),
    kind TEXT NOT NULL CONSTRAINT running_span_kind_known CHECK (kind IN ('held', 'waiting')),
    since_at INTEGER NOT NULL,
    PRIMARY KEY (item_id, kind)
) STRICT, WITHOUT ROWID;

-- The spans that ended, each within one run of its item.
CREATE TABLE item_span (
    id INTEGER PRIMARY KEY,
    item_id INTEGER NOT NULL REFERENCES item (id),
    kind TEXT NOT NULL CONSTRAINT item_span_kind_known CHECK (kind IN ('held', 'waiting')),
    since_at INTEGER NOT NULL,
    ended_at INTEGER NOT NULL,
    CONSTRAINT item_span_ends_after_it_starts CHECK (ended_at >= since_at)
) STRICT;
CREATE INDEX item_span_item ON item_span (item_id);

-- When the store began to record spans. The store move that made the span
-- tables writes this row in each store it moves, which holds no spans from
-- before that moment. A store with no row has recorded spans since builds
-- made it.
CREATE TABLE spans_recorded (
    singleton INTEGER PRIMARY KEY CONSTRAINT spans_recorded_is_single CHECK (singleton = 1),
    from_at INTEGER NOT NULL
) STRICT;

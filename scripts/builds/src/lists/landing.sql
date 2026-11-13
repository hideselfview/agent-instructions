-- The parts of an item that already landed (`--part`). landing.rs owns these
-- tables and is the only code that writes them.

CREATE TABLE item_landing (
    id INTEGER PRIMARY KEY,
    item_id INTEGER NOT NULL REFERENCES item (id),
    repo TEXT NOT NULL REFERENCES repo (name),
    at INTEGER NOT NULL
) STRICT;
CREATE INDEX item_landing_item ON item_landing (item_id);

CREATE TABLE item_landing_commit (
    landing_id INTEGER NOT NULL REFERENCES item_landing (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CONSTRAINT item_landing_commit_position_not_negative CHECK (position >= 0),
    sha TEXT NOT NULL CONSTRAINT item_landing_commit_sha_is_a_commit CHECK (length(sha) = 40 AND sha NOT GLOB '*[^0-9a-f]*'),
    PRIMARY KEY (landing_id, position),
    CONSTRAINT item_landing_commit_once UNIQUE (landing_id, sha)
) STRICT, WITHOUT ROWID;

-- What someone changed in an open item that its agent has to read before the
-- work lands: an edit of its details, with the details before and after, and
-- a decision that became its origin. item_change.rs owns these tables and is
-- the only code that writes them. A landing that finishes an item refuses
-- while a change made after the item's take has no row in `item_change_read`.

CREATE TABLE item_change (
    id INTEGER PRIMARY KEY,
    item_id INTEGER NOT NULL REFERENCES item (id),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT item_change_by_says_something CHECK (trim(by) <> ''),
    kind TEXT NOT NULL CONSTRAINT item_change_kind_known CHECK (kind IN ('details', 'decision')),
    -- The details of a `details` change, before and after.
    old_details TEXT,
    new_details TEXT,
    -- The decision a `decision` change made the item's origin.
    decision_id INTEGER REFERENCES decision (id),
    CONSTRAINT item_change_fits_its_kind CHECK (
        CASE kind
            WHEN 'details' THEN old_details IS NOT NULL AND new_details IS NOT NULL
                AND old_details <> new_details AND decision_id IS NULL
            ELSE old_details IS NULL AND new_details IS NULL AND decision_id IS NOT NULL
        END
    )
) STRICT;
CREATE INDEX item_change_item ON item_change (item_id);

-- The agent's acknowledgement that it read a change (`builds item changes`).
CREATE TABLE item_change_read (
    change_id INTEGER PRIMARY KEY REFERENCES item_change (id),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT item_change_read_by_says_something CHECK (trim(by) <> '')
) STRICT;

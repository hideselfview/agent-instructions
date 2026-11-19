-- When each reviewer process of a review began and how long it ran, which
-- shows where a review's time went. exchange_time.rs owns this table and is
-- the only code that writes it; record.rs writes it with the exchange it
-- times. A call's time runs from its first exchange's start to its last
-- exchange's end, and a run's from its `at` to its last call's end. An
-- exchange that builds recorded before it timed them has no row.

CREATE TABLE review_exchange_time (
    exchange_id INTEGER PRIMARY KEY REFERENCES review_exchange (id),
    -- Seconds since 1970.
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    ms INTEGER NOT NULL CHECK (ms >= 0)
) STRICT;

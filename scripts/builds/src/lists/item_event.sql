-- The moments an open item changed state or gained or lost a wait, kept to
-- show where a card's time went (`builds item times`). item_event.rs owns
-- these tables and is the only code that writes them. Every write of the
-- store ends by comparing each item with its last events and recording what
-- changed, in the write's own transaction (store.rs), so an event carries the
-- time of the write that caused it. Taking, stopping and closing an item
-- stay in `item_run` and `closing`, and held and waiting time in `item_span`.

CREATE TABLE item_event (
    id INTEGER PRIMARY KEY,
    item_id INTEGER NOT NULL REFERENCES item (id),
    at INTEGER NOT NULL,
    -- `ready`: an agent can start the item. `blocked`: an unmet wait, or
    -- being placed last, keeps it from starting. `running`: an agent took it.
    -- `backlog`: the item waits until the person starts it; detail is why.
    -- `wait-gained` and `wait-lost`: an unmet wait began or ended, by an edit
    -- of the item or because the record it waits on closed; detail names the
    -- wait as `item:N`, `question:N`, `design:N` or `condition:TEXT`.
    -- `handed-off`: the item's run went to another agent's brief; detail is
    -- the brief.
    kind TEXT NOT NULL CONSTRAINT item_event_kind_known CHECK (
        kind IN ('ready', 'blocked', 'running', 'backlog', 'wait-gained', 'wait-lost', 'handed-off')
    ),
    detail TEXT CONSTRAINT item_event_detail_says_something CHECK (detail IS NULL OR trim(detail) <> '')
) STRICT;
CREATE INDEX item_event_item ON item_event (item_id);

-- When the store began to record events. The store move that made the table
-- writes this row in each store it moves, and an item opened before then has
-- no events. A store with no row has recorded events since builds made it.
CREATE TABLE item_events_recorded (
    singleton INTEGER PRIMARY KEY CONSTRAINT item_events_recorded_is_single CHECK (singleton = 1),
    from_at INTEGER NOT NULL
) STRICT;

-- Each change of the state a side worktree's card shows on the dashboard, kept
-- to show where a card's time went (`builds item times`). card_state.rs owns
-- these tables and is the only code that writes them. The dashboard daemon
-- judges every card with the code that draws it (daemon/model.rs judge_card)
-- and writes a row to the store of the card's project whenever the card's state
-- or the items it lists differ from its last row there. A state holds from its
-- row's time until the next row of the same worktree. Each write deletes the
-- rows older than 30 days, except each worktree's last.

CREATE TABLE card_state (
    id INTEGER PRIMARY KEY,
    -- The worktree's top folder.
    worktree TEXT NOT NULL CHECK (substr(worktree, 1, 1) = '/'),
    -- When the daemon saw the change, seconds since 1970.
    at INTEGER NOT NULL CHECK (at > 0),
    -- The card's state (cards.rs CardState::name). `gone`: the card no longer
    -- shows under this project, because the worktree is gone or its lists are
    -- another project's or none.
    state TEXT NOT NULL CHECK (state IN (
        'free', 'working', 'winding-down', 'waiting-on-mac', 'waiting-on-android',
        'waiting-on-you', 'paused', 'ready-to-land', 'needs-review', 'reviewing',
        'findings-to-settle', 'open-findings', 'unreviewed-changes', 'waits-unread',
        'kept', 'kept-waiting', 'kept-met', 'gone'
    ))
) STRICT;
CREATE INDEX card_state_worktree ON card_state (worktree, id);

-- The items the card listed in that state: its running items, or the item
-- that keeps the worktree with no agent.
CREATE TABLE card_state_item (
    state_id INTEGER NOT NULL REFERENCES card_state (id) ON DELETE CASCADE,
    item_id INTEGER NOT NULL REFERENCES item (id),
    PRIMARY KEY (state_id, item_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX card_state_item_item ON card_state_item (item_id);

-- The work queue's items. item.rs owns these tables and is the only code
-- that writes them. Where an item runs is running.sql's, what already
-- landed of it landing.sql's.

CREATE TABLE item (
    id INTEGER PRIMARY KEY,
    record_kind TEXT NOT NULL DEFAULT 'item' CONSTRAINT item_is_an_item CHECK (record_kind = 'item'),
    title TEXT NOT NULL CONSTRAINT item_title_is_one_line CHECK (
        trim(title) <> '' AND title = trim(title)
        AND instr(title, char(9)) = 0 AND instr(title, char(10)) = 0 AND instr(title, char(13)) = 0
    ),
    -- A rename carries every item, open or closed.
    area TEXT NOT NULL REFERENCES area (name) ON UPDATE CASCADE,
    details TEXT NOT NULL CONSTRAINT item_details_say_something CHECK (trim(details) <> ''),
    audience TEXT NOT NULL CONSTRAINT item_audience_known CHECK (audience IN ('coordinator', 'design-session')),
    brief TEXT CONSTRAINT item_brief_says_something CHECK (brief IS NULL OR trim(brief) <> ''),
    -- The record it came from, of any kind (trail.rs).
    origin INTEGER,
    CONSTRAINT item_origin_not_itself CHECK (origin <> id),
    CONSTRAINT item_is_a_record FOREIGN KEY (id, record_kind) REFERENCES record (id, kind),
    CONSTRAINT item_origin_is_a_record FOREIGN KEY (origin) REFERENCES record (id)
) STRICT;

CREATE INDEX item_origin ON item (origin);
CREATE INDEX item_area ON item (area);

CREATE TABLE item_priority (
    item_id INTEGER PRIMARY KEY REFERENCES item (id) ON DELETE CASCADE,
    level TEXT NOT NULL CONSTRAINT item_priority_level_known CHECK (level IN ('first', 'last')),
    why TEXT NOT NULL CONSTRAINT item_priority_why_says_something CHECK (trim(why) <> '')
) STRICT;

-- An open item is in the backlog while its row exists: work that waits until
-- the person starts it. Starting the item deletes the row. A closing leaves
-- it, so a closed item shows that it closed from the backlog.
CREATE TABLE item_backlog (
    item_id INTEGER PRIMARY KEY REFERENCES item (id),
    why TEXT NOT NULL CONSTRAINT item_backlog_why_says_something CHECK (trim(why) <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT item_backlog_by_says_something CHECK (trim(by) <> '')
) STRICT;

-- What an item waits on, one table per kind of wait.
CREATE TABLE item_wait_item (
    item_id INTEGER NOT NULL REFERENCES item (id),
    on_item INTEGER NOT NULL REFERENCES item (id),
    PRIMARY KEY (item_id, on_item),
    CONSTRAINT item_wait_item_not_itself CHECK (item_id <> on_item)
) STRICT, WITHOUT ROWID;
CREATE INDEX item_wait_item_target ON item_wait_item (on_item);

CREATE TABLE item_wait_question (
    item_id INTEGER NOT NULL REFERENCES item (id),
    on_question INTEGER NOT NULL REFERENCES question (id),
    PRIMARY KEY (item_id, on_question)
) STRICT, WITHOUT ROWID;
CREATE INDEX item_wait_question_target ON item_wait_question (on_question);

CREATE TABLE item_wait_design (
    item_id INTEGER NOT NULL REFERENCES item (id),
    on_design INTEGER NOT NULL REFERENCES design (id),
    PRIMARY KEY (item_id, on_design)
) STRICT, WITHOUT ROWID;
CREATE INDEX item_wait_design_target ON item_wait_design (on_design);

-- A condition is met only when its ender removes it. The person is never an
-- ender: work that waits on the person waits on a your-step question.
CREATE TABLE item_wait_condition (
    item_id INTEGER NOT NULL REFERENCES item (id),
    condition TEXT NOT NULL CONSTRAINT item_wait_condition_says_something CHECK (trim(condition) <> ''),
    until TEXT NOT NULL CONSTRAINT item_wait_condition_ender_known CHECK (until IN ('agent', 'coordinator', 'ci')),
    PRIMARY KEY (item_id, condition)
) STRICT, WITHOUT ROWID;

-- A progress line; the newest has the highest id.
CREATE TABLE item_note (
    id INTEGER PRIMARY KEY,
    item_id INTEGER NOT NULL REFERENCES item (id),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT item_note_by_says_something CHECK (trim(by) <> ''),
    text TEXT NOT NULL CONSTRAINT item_note_says_something CHECK (trim(text) <> '')
) STRICT;
CREATE INDEX item_note_item ON item_note (item_id);

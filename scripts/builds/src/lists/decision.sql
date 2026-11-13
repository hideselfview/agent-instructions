-- The decision log. decision.rs owns these tables and is the only code that
-- writes them. Never stored: the question a decision answered (the closing
-- that names it), the items it made (the items whose origin it is) and what
-- replaced it (the decisions that name it).

CREATE TABLE decision (
    id INTEGER PRIMARY KEY,
    record_kind TEXT NOT NULL DEFAULT 'decision' CONSTRAINT decision_is_a_decision CHECK (record_kind = 'decision'),
    date TEXT NOT NULL CONSTRAINT decision_date_is_a_day CHECK (date(date) IS date),
    title TEXT NOT NULL CONSTRAINT decision_title_is_one_line CHECK (
        trim(title) <> '' AND title = trim(title)
        AND instr(title, char(9)) = 0 AND instr(title, char(10)) = 0 AND instr(title, char(13)) = 0
    ),
    decided_by TEXT NOT NULL CONSTRAINT decision_decided_by_says_something CHECK (trim(decided_by) <> ''),
    body TEXT NOT NULL CONSTRAINT decision_body_says_something CHECK (trim(body) <> ''),
    -- A decision revises only an older one, and `replaces` is never edited,
    -- so a chain of revisions can never close into a circle.
    replaces INTEGER CONSTRAINT decision_replaces_an_older_one CHECK (replaces < id) REFERENCES decision (id),
    CONSTRAINT decision_is_a_record FOREIGN KEY (id, record_kind) REFERENCES record (id, kind)
) STRICT;

-- The plan of a decision that answers no question; an answering decision's
-- plan is its question's.
CREATE TABLE decision_plan (
    decision_id INTEGER PRIMARY KEY REFERENCES decision (id) ON DELETE CASCADE,
    path TEXT NOT NULL CONSTRAINT decision_plan_path_says_something CHECK (trim(path) <> ''),
    number TEXT NOT NULL CONSTRAINT decision_plan_number_says_something CHECK (trim(number) <> '')
) STRICT;

-- The decision stack's questions. question.rs owns these tables and is the
-- only code that writes them. What a question blocks is not stored: it is
-- the open items whose waits name it.

CREATE TABLE question (
    id INTEGER PRIMARY KEY,
    record_kind TEXT NOT NULL DEFAULT 'question' CONSTRAINT question_is_a_question CHECK (record_kind = 'question'),
    title TEXT NOT NULL CONSTRAINT question_title_is_one_line CHECK (
        trim(title) <> '' AND title = trim(title)
        AND instr(title, char(9)) = 0 AND instr(title, char(10)) = 0 AND instr(title, char(13)) = 0
    ),
    details TEXT NOT NULL CONSTRAINT question_details_say_something CHECK (trim(details) <> ''),
    kind TEXT NOT NULL CONSTRAINT question_kind_known CHECK (kind IN ('question', 'your-step', 'held')),
    -- Why it is asked though nothing waits on it.
    blocks_nothing TEXT CONSTRAINT question_blocks_nothing_says_something CHECK (blocks_nothing IS NULL OR trim(blocks_nothing) <> ''),
    CONSTRAINT question_is_a_record FOREIGN KEY (id, record_kind) REFERENCES record (id, kind)
) STRICT;

-- Where a question sits in the stack when not by how much it unblocks.
CREATE TABLE question_placement (
    question_id INTEGER PRIMARY KEY REFERENCES question (id) ON DELETE CASCADE,
    at TEXT NOT NULL CONSTRAINT question_placement_at_known CHECK (at IN ('first', 'last')),
    why TEXT NOT NULL CONSTRAINT question_placement_why_says_something CHECK (trim(why) <> ''),
    by_person INTEGER NOT NULL CONSTRAINT question_placement_by_person_is_a_flag CHECK (by_person IN (0, 1))
) STRICT;

-- A plan's numbered open question.
CREATE TABLE question_plan (
    question_id INTEGER PRIMARY KEY REFERENCES question (id) ON DELETE CASCADE,
    path TEXT NOT NULL CONSTRAINT question_plan_path_says_something CHECK (trim(path) <> ''),
    number TEXT NOT NULL CONSTRAINT question_plan_number_says_something CHECK (trim(number) <> '')
) STRICT;

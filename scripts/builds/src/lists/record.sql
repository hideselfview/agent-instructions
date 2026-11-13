-- What every record shares: one id space for questions, items, design
-- changes and decisions, who opened each and when, and how a record left its
-- list. record.rs owns these tables and is the only code that writes them.
-- Every kind's table points at `record (id, kind)` with its own kind fixed,
-- so a reference to "an item" can only name an item.

CREATE TABLE record (
    id INTEGER PRIMARY KEY CONSTRAINT record_id_positive CHECK (id >= 1),
    kind TEXT NOT NULL CONSTRAINT record_kind_known CHECK (kind IN ('question', 'item', 'design', 'decision')),
    opened_at INTEGER NOT NULL,
    opened_by TEXT NOT NULL CONSTRAINT record_opened_by_says_something CHECK (trim(opened_by) <> ''),
    CONSTRAINT record_id_with_kind UNIQUE (id, kind)
) STRICT;

-- One row per record that left its list. The record keeps its own rows.
CREATE TABLE closing (
    record_id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    how TEXT NOT NULL CONSTRAINT closing_how_known CHECK (
        how IN ('answered', 'settled', 'done', 'landed', 'dropped', 'deferred', 'moved-to-stack')
    ),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT closing_by_says_something CHECK (trim(by) <> ''),
    -- The decision that answered a question: the one place that says so.
    decision_id INTEGER CONSTRAINT closing_decision_answers_once UNIQUE REFERENCES decision (id),
    -- The question that took an item's place.
    question_id INTEGER REFERENCES question (id),
    reason TEXT,
    CONSTRAINT closing_names_its_record FOREIGN KEY (record_id, kind) REFERENCES record (id, kind),
    CONSTRAINT closing_how_fits_kind CHECK (
        CASE kind
            WHEN 'question' THEN how IN ('answered', 'settled', 'done', 'dropped', 'deferred')
            WHEN 'item' THEN how IN ('landed', 'dropped', 'deferred', 'moved-to-stack')
            WHEN 'design' THEN how = 'landed'
            ELSE 0
        END
    ),
    CONSTRAINT closing_answered_names_its_decision CHECK ((how = 'answered') = (decision_id IS NOT NULL)),
    CONSTRAINT closing_moved_names_its_question CHECK ((how = 'moved-to-stack') = (question_id IS NOT NULL)),
    CONSTRAINT closing_reason_goes_with_how CHECK ((how IN ('settled', 'dropped', 'deferred')) = (reason IS NOT NULL)),
    CONSTRAINT closing_reason_says_something CHECK (reason IS NULL OR trim(reason) <> ''),
    CONSTRAINT closing_with_how UNIQUE (record_id, how)
) STRICT;

CREATE TABLE closing_commit (
    record_id INTEGER NOT NULL,
    how TEXT NOT NULL CONSTRAINT closing_commit_of_a_landing CHECK (how = 'landed'),
    position INTEGER NOT NULL CONSTRAINT closing_commit_position_not_negative CHECK (position >= 0),
    sha TEXT NOT NULL CONSTRAINT closing_commit_sha_is_a_commit CHECK (length(sha) = 40 AND sha NOT GLOB '*[^0-9a-f]*'),
    PRIMARY KEY (record_id, position),
    CONSTRAINT closing_commit_once UNIQUE (record_id, sha),
    CONSTRAINT closing_commit_belongs_to_a_landed_closing FOREIGN KEY (record_id, how) REFERENCES closing (record_id, how)
) STRICT, WITHOUT ROWID;

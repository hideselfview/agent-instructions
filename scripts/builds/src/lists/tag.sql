-- The tags of questions and items: each names the audit or group a record came
-- from, such as "screen-reads audit", so the person can see one group's
-- records together. tag.rs owns this table and is the only code that writes
-- it. A record keeps its tags when it closes.

CREATE TABLE record_tag (
    record_id INTEGER NOT NULL,
    kind TEXT NOT NULL CONSTRAINT record_tag_of_a_question_or_an_item CHECK (kind IN ('question', 'item')),
    tag TEXT NOT NULL CONSTRAINT record_tag_is_one_line CHECK (
        trim(tag) <> '' AND tag = trim(tag)
        AND instr(tag, char(9)) = 0 AND instr(tag, char(10)) = 0 AND instr(tag, char(13)) = 0
    ),
    PRIMARY KEY (record_id, tag),
    CONSTRAINT record_tag_names_its_record FOREIGN KEY (record_id, kind) REFERENCES record (id, kind)
) STRICT, WITHOUT ROWID;
CREATE INDEX record_tag_tag ON record_tag (tag);

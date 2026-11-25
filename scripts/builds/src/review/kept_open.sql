-- A reviewer's accepted verdict that an open finding still applies, with the
-- reason it gave. Later runs use these rows to know which changes the group
-- read with the finding. A row recorded before builds kept the reason has
-- none.
CREATE TABLE review_kept_open (
    call_id INTEGER NOT NULL REFERENCES review_call (id),
    finding_id INTEGER NOT NULL REFERENCES review_finding (id),
    reason TEXT CONSTRAINT review_kept_open_reason_says_something CHECK (reason IS NULL OR trim(reason) <> ''),
    PRIMARY KEY (call_id, finding_id)
) STRICT, WITHOUT ROWID;

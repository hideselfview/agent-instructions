-- The tell of each matter that waits for the judge (judge_told.rs owns this
-- table and is the only code that writes it): the finding that waits to be
-- settled, or the dispute that waits to be answered, with the builds process
-- that is to tell the judge of it and the judge session builds told.

-- The process that makes a finding wait writes its row in the same
-- transaction, naming itself by its pid and start time, so that a row of a
-- process that no longer runs shows that nothing is telling the judge of the
-- finding. Telling a judge's session of the matter marks the row told. A
-- matter that waits and has no row, or whose row names another session than
-- the listening judge's, and whose process no longer runs, is a matter that
-- no one is telling the judge of.
CREATE TABLE review_judge_told (
    finding_id INTEGER NOT NULL REFERENCES review_finding (id),
    matter TEXT NOT NULL CONSTRAINT review_judge_told_matter_known CHECK (matter IN ('settle', 'answer')),
    -- The builds process that wrote the row and its start time in seconds
    -- since 1970. A process is the same one only when both match.
    writer_pid INTEGER NOT NULL,
    writer_started INTEGER NOT NULL,
    -- The judge session builds told of the matter, and when.
    told_session TEXT CONSTRAINT review_judge_told_session_is_named CHECK (told_session IS NULL OR told_session <> ''),
    told_at INTEGER,
    PRIMARY KEY (finding_id, matter),
    CONSTRAINT review_judge_told_whole CHECK ((told_session IS NULL) = (told_at IS NULL))
) STRICT;

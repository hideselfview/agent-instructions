-- What builds told the judge (judge_told.rs owns this table and is the only
-- code that writes it): each finding that waited for the judge, as the
-- session that judged then was told of it.

-- A row for each matter that a finding put to a judge: a finding to settle,
-- or the dispute of a confirmed finding to answer. The row names the judge's
-- session that builds told, so a judge that listens later, in another
-- session, has been told of nothing yet.
CREATE TABLE review_judge_told (
    finding_id INTEGER NOT NULL REFERENCES review_finding (id),
    matter TEXT NOT NULL CONSTRAINT review_judge_told_matter_known CHECK (matter IN ('settle', 'answer')),
    session TEXT NOT NULL CONSTRAINT review_judge_told_session_is_named CHECK (session <> ''),
    at INTEGER NOT NULL,
    PRIMARY KEY (finding_id, matter)
) STRICT;

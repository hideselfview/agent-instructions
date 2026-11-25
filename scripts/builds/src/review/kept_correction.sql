-- The judge's correction of the reason it gave when it kept a disputed
-- finding (`review_kept`). The kept row stays as the judge recorded it, and the
-- latest correction is the reason builds shows.
CREATE TABLE review_kept_correction (
    id INTEGER PRIMARY KEY,
    finding_id INTEGER NOT NULL REFERENCES review_kept (finding_id),
    why TEXT NOT NULL CONSTRAINT review_kept_correction_why_says_something CHECK (trim(why) <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_kept_correction_by_says_something CHECK (trim(by) <> '')
) STRICT;

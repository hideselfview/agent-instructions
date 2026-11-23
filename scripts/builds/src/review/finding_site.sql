-- Builds records the side of the diff where it verified the quote. NULL
-- marks a finding from before Builds recorded where it found the quote.
ALTER TABLE review_finding ADD COLUMN site_kind TEXT
    CONSTRAINT review_finding_site_kind_known CHECK (site_kind IN ('head', 'removed', 'reported'));

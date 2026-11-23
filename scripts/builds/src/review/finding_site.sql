-- Builds records the side of the diff where it verified the quote. NULL
-- marks a finding from before Builds recorded where it found the quote.
ALTER TABLE review_finding ADD COLUMN site_kind TEXT
    CONSTRAINT review_finding_site_kind_known CHECK (site_kind IN ('head', 'removed', 'reported'));

-- The quote's path on that side, which can differ from the file's new name.
ALTER TABLE review_finding ADD COLUMN site_path TEXT
    CONSTRAINT review_finding_site_path_matches_kind CHECK (
        CASE WHEN site_kind IN ('head', 'removed') THEN site_path IS NOT NULL AND site_path <> ''
             ELSE site_path IS NULL
        END
    );

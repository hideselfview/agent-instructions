-- Builds keeps whether it verified the quote at the reviewed head. NULL
-- marks a finding from before Builds recorded where it found the quote.
ALTER TABLE review_finding ADD COLUMN head_site INTEGER
    CONSTRAINT review_finding_head_site_is_a_flag CHECK (head_site IN (0, 1));

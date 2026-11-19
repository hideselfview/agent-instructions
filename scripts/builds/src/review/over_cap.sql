-- A branch whose diff adds more lines than a review sends its reviewers
-- (`builds review`, review/over_cap.rs owns this table and is the only code
-- that writes it). The branch's net change is named by its patch id, so a
-- rebase that keeps its lines keeps the row. A change of design files or of
-- renamed identifiers needs no review (mechanical). Any other change waits on
-- a question to the person, which holds the branch's name and size.
CREATE TABLE review_over_cap (
    id INTEGER PRIMARY KEY,
    repo TEXT NOT NULL REFERENCES repo (name),
    key TEXT NOT NULL CONSTRAINT review_over_cap_key_says_something CHECK (trim(key) <> ''),
    patch_id TEXT NOT NULL CONSTRAINT review_over_cap_patch_id_is_a_hash CHECK (length(patch_id) = 40 AND patch_id NOT GLOB '*[^0-9a-f]*'),
    added_lines INTEGER NOT NULL CONSTRAINT review_over_cap_adds_lines CHECK (added_lines > 0),
    cap INTEGER NOT NULL CONSTRAINT review_over_cap_cap_is_positive CHECK (cap > 0),
    at INTEGER NOT NULL,
    mechanical TEXT CONSTRAINT review_over_cap_mechanical_known CHECK (mechanical IN ('design', 'rename')),
    question_id INTEGER REFERENCES question (id),
    CONSTRAINT review_over_cap_needs_no_review_or_asks CHECK ((mechanical IS NULL) <> (question_id IS NULL)),
    CONSTRAINT review_over_cap_once UNIQUE (repo, key, patch_id)
) STRICT;

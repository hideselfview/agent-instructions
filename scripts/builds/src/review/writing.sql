-- Each row records one run of the writing check (`builds review writing`)
-- that its group answered: the change's key and repo, the head the check
-- read, and the newest review run of a landing with that key when the check
-- ran. builds allows two such runs on a key between two reviews of it. writing.rs owns this table and is
-- the only code that writes it.
CREATE TABLE review_writing_run (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL CONSTRAINT review_writing_run_key_says_something CHECK (trim(key) <> ''),
    repo TEXT NOT NULL REFERENCES repo (name),
    head TEXT NOT NULL,
    -- The column is null when no review had run on the key.
    reviewed_run INTEGER REFERENCES review_run (id),
    at INTEGER NOT NULL
) STRICT;
CREATE INDEX review_writing_run_key ON review_writing_run (key);

-- A branch an agent called mechanical work (`builds review --kind
-- mechanical`, review/marked_mechanical.rs owns this table and is the only code that
-- writes it): moved or renamed code, regenerated files or a codemod, which
-- the compiler, the hook and a machine check cover. `by` names who called it
-- and `checked_by` the check that covers it. The branch's net change is
-- named by its patch id, so a rebase that keeps its lines keeps the row.
-- `builds land` takes such a branch with no review.
CREATE TABLE review_marked_mechanical (
    id INTEGER PRIMARY KEY,
    repo TEXT NOT NULL REFERENCES repo (name),
    key TEXT NOT NULL CONSTRAINT review_marked_mechanical_key_says_something CHECK (trim(key) <> ''),
    patch_id TEXT NOT NULL CONSTRAINT review_marked_mechanical_patch_id_is_a_hash CHECK (length(patch_id) = 40 AND patch_id NOT GLOB '*[^0-9a-f]*'),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_marked_mechanical_by_says_something CHECK (trim(by) <> ''),
    checked_by TEXT NOT NULL CONSTRAINT review_marked_mechanical_checked_by_says_something CHECK (trim(checked_by) <> ''),
    CONSTRAINT review_marked_mechanical_once UNIQUE (repo, key, patch_id)
) STRICT;

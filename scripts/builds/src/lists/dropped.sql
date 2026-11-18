-- These tables record the work an item held when the person dropped it
-- (`builds item drop`): for each worktree it ran in, the branch,
-- the commits on it that origin/main lacked, and the files nobody had
-- committed. The drop
-- deletes the branch and discards the files, so these tables record what the
-- drop removed; the commits stay in git until it removes unreachable objects.
-- dropped.rs owns these tables and is the only code that writes them.

CREATE TABLE dropped_place (
    item_id INTEGER NOT NULL REFERENCES item (id),
    repo TEXT NOT NULL REFERENCES repo (name),
    worktree TEXT NOT NULL CONSTRAINT dropped_place_worktree_is_absolute CHECK (substr(worktree, 1, 1) = '/'),
    branch TEXT NOT NULL CONSTRAINT dropped_place_branch_says_something CHECK (trim(branch) <> ''),
    PRIMARY KEY (item_id, repo)
) STRICT, WITHOUT ROWID;

CREATE TABLE dropped_commit (
    item_id INTEGER NOT NULL,
    repo TEXT NOT NULL,
    position INTEGER NOT NULL CONSTRAINT dropped_commit_position_not_negative CHECK (position >= 0),
    sha TEXT NOT NULL CONSTRAINT dropped_commit_sha_is_a_commit CHECK (length(sha) = 40 AND sha NOT GLOB '*[^0-9a-f]*'),
    subject TEXT NOT NULL,
    PRIMARY KEY (item_id, repo, position),
    FOREIGN KEY (item_id, repo) REFERENCES dropped_place (item_id, repo)
) STRICT, WITHOUT ROWID;

CREATE TABLE dropped_file (
    item_id INTEGER NOT NULL,
    repo TEXT NOT NULL,
    path TEXT NOT NULL CONSTRAINT dropped_file_path_says_something CHECK (path <> ''),
    PRIMARY KEY (item_id, repo, path),
    FOREIGN KEY (item_id, repo) REFERENCES dropped_place (item_id, repo)
) STRICT, WITHOUT ROWID;

-- The commit each `builds cargo` run ran on. cargo_run_commit.rs owns this
-- table and is the only code that writes it; cargo_run.rs writes it in the
-- transaction that writes the run. A run that builds recorded before it kept
-- commits, or whose worktree git could not read, has no row.

CREATE TABLE builds_cargo_run_commit (
    run_id INTEGER PRIMARY KEY REFERENCES builds_cargo_runs (id) ON DELETE CASCADE,
    -- HEAD's commit when the run started.
    head TEXT NOT NULL CHECK (length(head) = 40 AND head NOT GLOB '*[^0-9a-f]*'),
    -- The branch checked out; NULL for a detached HEAD.
    branch TEXT CHECK (branch <> ''),
    -- A hash of the uncommitted changes, each changed path with its content
    -- (tree_state.rs); NULL for a clean tree.
    changes TEXT CHECK (changes <> ''),
    -- The branch of the item that ran in the worktree, and whether HEAD was
    -- off it: on neither that branch nor origin/main since the branch left it
    -- (a bisect or an old checkout). Both NULL when no item ran there, and
    -- off_branch NULL when git could not tell.
    run_branch TEXT CHECK (run_branch <> ''),
    off_branch INTEGER CHECK (off_branch IN (0, 1)),
    CHECK (run_branch IS NOT NULL OR off_branch IS NULL)
) STRICT;

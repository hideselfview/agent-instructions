-- The findings a `builds app-build ... --shots` run printed, kept so that the
-- next agent that wants them for the same commit reads them instead of
-- building. shot_findings.rs owns this table and is the only code that writes
-- it. A row is a fact a run observed: the text the shots program printed.

-- One row per commit, platform, shot-name prefix and version of the shots
-- program's source; a later run with the same key replaces the row.
CREATE TABLE IF NOT EXISTS shot_findings (
    repo TEXT NOT NULL CHECK (repo <> ''),
    -- The full id of the commit the run built.
    commit_id TEXT NOT NULL CHECK (commit_id <> ''),
    platform TEXT NOT NULL CHECK (platform IN ('mac', 'android')),
    -- The shot-name prefix the run was given (`mac.welcome.`).
    prefix TEXT NOT NULL CHECK (prefix <> ''),
    -- The id of the git tree of the shots program's source folder
    -- (`shots-source` in .app-build) at that commit, so that findings from
    -- another version of the checker are never taken for these.
    shots_version TEXT NOT NULL CHECK (shots_version <> ''),
    -- When the run ended, seconds since 1970.
    recorded_at INTEGER NOT NULL CHECK (recorded_at > 0),
    -- The shots command's exit status: 0 when no pair failed the run.
    exit_code INTEGER NOT NULL CHECK (exit_code >= 0),
    -- What the shots program printed once it started, findings and verdict.
    findings TEXT NOT NULL,
    PRIMARY KEY (repo, commit_id, platform, prefix, shots_version)
) STRICT, WITHOUT ROWID;

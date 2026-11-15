-- The record of `builds cargo` runs, in `builds.sqlite` in a project's store
-- folder. cargo_runs.rs owns this table and is the only code that writes it.
-- A row is a fact a run observed: which host ran the build, when it started,
-- and how it ended. The dashboard's host rows read it back.

-- One row per `builds cargo` run, written when the run starts and finished
-- when it ends. A run that dies without ending (a killed `builds`) keeps no
-- end; the dashboard counts a run as running only while its `pid` is alive.
CREATE TABLE IF NOT EXISTS cargo_runs (
    id INTEGER PRIMARY KEY,
    -- When the run started, seconds since 1970.
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    -- When it ended; NULL while it runs.
    ended_at INTEGER CHECK (ended_at >= started_at),
    repo TEXT NOT NULL CHECK (repo <> ''),
    -- The checkout (main, side-N) whose files the run built.
    checkout TEXT NOT NULL CHECK (checkout <> ''),
    -- The host that ran cargo: `local` for this Mac, else a host's name.
    host TEXT NOT NULL CHECK (host <> ''),
    -- The `builds cargo` process on this Mac that started the run.
    pid INTEGER NOT NULL CHECK (pid > 0),
    -- cargo's subcommand and the arguments the caller gave.
    command TEXT NOT NULL CHECK (command <> ''),
    -- cargo's exit status; NULL while it runs.
    exit_code INTEGER,
    CHECK ((ended_at IS NULL) = (exit_code IS NULL))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_cargo_runs_started ON cargo_runs (started_at);

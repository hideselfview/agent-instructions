-- What each build host said about its load and memory when the dashboard
-- daemon asked it, about every 30 seconds, kept so a step's time can be read
-- beside its host's load (`builds item times`). host_sample.rs owns this table
-- and is the only code that writes it. Every project's store holds the same
-- rows, since the hosts are the machine's, not a project's, and each store
-- keeps the rows of the last 30 days. A host that did not answer has no row
-- for that moment.

CREATE TABLE host_sample (
    -- `local` for this Mac, else the host's name.
    host TEXT NOT NULL CHECK (host <> ''),
    -- When the host answered, seconds since 1970.
    at INTEGER NOT NULL CHECK (at > 0),
    cores INTEGER NOT NULL CHECK (cores > 0),
    -- The 1-minute load average.
    load_1 REAL NOT NULL CHECK (load_1 >= 0),
    -- The kernel's memory pressure level.
    memory TEXT NOT NULL CHECK (memory IN ('normal', 'warning', 'critical')),
    PRIMARY KEY (host, at)
) STRICT, WITHOUT ROWID;

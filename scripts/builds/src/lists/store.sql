-- The write log: one row per write, inserted by the write's own transaction.
-- store.rs owns this table and is the only code that writes it. Nothing is
-- derived from the log; the lists' version is its last row's.

CREATE TABLE write_log (
    version INTEGER PRIMARY KEY CONSTRAINT write_log_version_positive CHECK (version >= 1),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT write_log_by_says_something CHECK (trim(by) <> ''),
    words TEXT NOT NULL
) STRICT;

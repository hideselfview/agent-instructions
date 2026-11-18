-- How the last check for a new stable Rust release ended in this project
-- (rust_check.rs owns this table and is the only code that writes it;
-- rust_stable.rs makes the checks). One row, replaced by each check: `fault`
-- is empty when the check read the manifest and every pin, and `builds
-- health` lists a fault until a later check succeeds.

CREATE TABLE rust_check (
    singleton INTEGER PRIMARY KEY CONSTRAINT rust_check_is_single CHECK (singleton = 1),
    at INTEGER NOT NULL,
    fault TEXT CONSTRAINT rust_check_fault_says_something CHECK (fault IS NULL OR trim(fault) <> '')
) STRICT;

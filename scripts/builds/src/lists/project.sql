-- The project: its name, the repos its lists cover, and each repo's globs for
-- design places, design-system sources and spec files. project.rs owns these
-- tables and is the only code that writes them.

CREATE TABLE project (
    singleton INTEGER PRIMARY KEY CONSTRAINT project_is_single CHECK (singleton = 1),
    name TEXT NOT NULL CONSTRAINT project_name_is_a_folder_name CHECK (
        name <> '' AND name NOT GLOB '.*' AND name NOT GLOB '*[^A-Za-z0-9._-]*'
    )
) STRICT;

CREATE TABLE repo (
    name TEXT PRIMARY KEY CONSTRAINT repo_name_is_a_directory_name CHECK (name <> '' AND name NOT GLOB '*/*')
) STRICT;

-- Per repo, the globs of one kind of design file: `place` (design places),
-- `source` (the shared design-system sources) and `spec` (the spec files).
CREATE TABLE design_glob (
    repo TEXT NOT NULL REFERENCES repo (name),
    kind TEXT NOT NULL CONSTRAINT design_glob_kind_known CHECK (kind IN ('place', 'source', 'spec')),
    glob TEXT NOT NULL CONSTRAINT design_glob_says_something CHECK (trim(glob) <> ''),
    PRIMARY KEY (repo, kind, glob)
) STRICT, WITHOUT ROWID;

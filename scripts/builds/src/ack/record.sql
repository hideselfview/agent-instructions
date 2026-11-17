-- `builds ack` records that an agent read the text builds asks every agent to
-- read before it runs a builds command. builds names an agent by its Claude
-- Code session and its name (`who`): the agents a session dispatches share its
-- session id, so the name tells them apart.

-- Each agent's last acknowledgement: the version of the text it read and when.
CREATE TABLE ack (
    session TEXT NOT NULL CONSTRAINT ack_session_is_named CHECK (session <> ''),
    who TEXT NOT NULL CONSTRAINT ack_who_is_named CHECK (who <> ''),
    version TEXT NOT NULL CONSTRAINT ack_version_is_named CHECK (version <> ''),
    at INTEGER NOT NULL,
    PRIMARY KEY (session, who)
) STRICT, WITHOUT ROWID;

-- The versions of the text that agents acknowledged most recently, so that
-- the reading of an agent whose last acknowledged version is here holds only
-- the sections that changed since. builds keeps `KEPT_VERSIONS` of them
-- (record.rs) and deletes the rest, so an `ack` row may name a version this
-- table no longer holds.
CREATE TABLE ack_version (
    version TEXT PRIMARY KEY CONSTRAINT ack_version_version_is_named CHECK (version <> ''),
    -- When an agent last acknowledged this version.
    acknowledged_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- The hash of each section of a kept version's text, by the section's name.
CREATE TABLE ack_version_section (
    version TEXT NOT NULL REFERENCES ack_version (version) ON DELETE CASCADE,
    name TEXT NOT NULL CONSTRAINT ack_version_section_name_is_named CHECK (name <> ''),
    hash TEXT NOT NULL CONSTRAINT ack_version_section_hash_is_named CHECK (hash <> ''),
    PRIMARY KEY (version, name)
) STRICT, WITHOUT ROWID;

-- The reading an agent started with `builds ack`: the version it reads. The
-- agent's next `builds ack` replaces it, and its acknowledgement deletes it.
CREATE TABLE ack_reading (
    session TEXT NOT NULL CONSTRAINT ack_reading_session_is_named CHECK (session <> ''),
    who TEXT NOT NULL CONSTRAINT ack_reading_who_is_named CHECK (who <> ''),
    version TEXT NOT NULL CONSTRAINT ack_reading_version_is_named CHECK (version <> ''),
    started_at INTEGER NOT NULL,
    PRIMARY KEY (session, who)
) STRICT, WITHOUT ROWID;

-- The sections a reading holds, by name, in the order its parts show them:
-- every section of the text, or only those that changed since the version
-- the agent last acknowledged.
CREATE TABLE ack_reading_section (
    session TEXT NOT NULL,
    who TEXT NOT NULL,
    position INTEGER NOT NULL CONSTRAINT ack_reading_section_position_counts_from_one CHECK (position >= 1),
    name TEXT NOT NULL CONSTRAINT ack_reading_section_name_is_named CHECK (name <> ''),
    PRIMARY KEY (session, who, position),
    UNIQUE (session, who, name),
    FOREIGN KEY (session, who) REFERENCES ack_reading (session, who) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- The check words of each part of a reading, in the order the part shows
-- them. The acknowledgement must give every one of them, in order.
CREATE TABLE ack_word (
    session TEXT NOT NULL,
    who TEXT NOT NULL,
    part INTEGER NOT NULL CONSTRAINT ack_word_part_counts_from_one CHECK (part >= 1),
    position INTEGER NOT NULL CONSTRAINT ack_word_position_counts_from_one CHECK (position >= 1),
    word TEXT NOT NULL CONSTRAINT ack_word_is_a_word CHECK (word <> '' AND word NOT GLOB '*[^a-z]*'),
    PRIMARY KEY (session, who, part, position),
    FOREIGN KEY (session, who) REFERENCES ack_reading (session, who) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- `builds ack` records that an agent read the text builds asks every agent to
-- read before it runs a builds command. builds names an agent by its Claude
-- Code session and its name (`who`): the agents a session dispatches share its
-- session id, so the name tells them apart.

-- Each agent's last acknowledgement: the version of the text it read and when.
-- After Claude Code compacts a session, builds deletes the session's rows.
CREATE TABLE ack (
    session TEXT NOT NULL CONSTRAINT ack_session_is_named CHECK (session <> ''),
    who TEXT NOT NULL CONSTRAINT ack_who_is_named CHECK (who <> ''),
    version TEXT NOT NULL CONSTRAINT ack_version_is_named CHECK (version <> ''),
    at INTEGER NOT NULL,
    PRIMARY KEY (session, who)
) STRICT, WITHOUT ROWID;

-- The hash of each section of the text that an acknowledgement covered, so
-- that a refusal names the sections that changed since.
CREATE TABLE ack_section (
    session TEXT NOT NULL,
    who TEXT NOT NULL,
    name TEXT NOT NULL CONSTRAINT ack_section_name_is_named CHECK (name <> ''),
    hash TEXT NOT NULL CONSTRAINT ack_section_hash_is_named CHECK (hash <> ''),
    PRIMARY KEY (session, who, name),
    FOREIGN KEY (session, who) REFERENCES ack (session, who) ON DELETE CASCADE
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

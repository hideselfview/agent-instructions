-- `builds ack` records that an agent read the text builds asks every agent to
-- read before it runs a builds command. builds names an agent by its Claude
-- Code session and the id Claude Code gives the agent (`agent`; agent.rs): the
-- agents a session dispatches share its session id, so the agent id tells
-- them apart.

-- Each agent's last acknowledgement of the text: the version of the text it
-- read.
CREATE TABLE ack (
    session TEXT NOT NULL CONSTRAINT ack_session_is_named CHECK (session <> ''),
    agent TEXT NOT NULL CONSTRAINT ack_agent_is_named CHECK (agent <> ''),
    version TEXT NOT NULL CONSTRAINT ack_version_is_named CHECK (version <> ''),
    PRIMARY KEY (session, agent)
) STRICT, WITHOUT ROWID;

-- The versions of the text that agents acknowledged most recently, so that
-- the notice to an agent whose last acknowledged version is here names only
-- the sections that changed since. builds keeps `KEPT_VERSIONS` of them
-- (record.rs) and deletes the rest, so an `ack` row may name a version this
-- table no longer holds.
CREATE TABLE ack_version (
    version TEXT PRIMARY KEY CONSTRAINT ack_version_version_is_named CHECK (version <> ''),
    -- When an agent last acknowledged this version.
    acknowledged_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- The hash of each section of the text of a version that builds keeps, by
-- the section's name.
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
    agent TEXT NOT NULL CONSTRAINT ack_reading_agent_is_named CHECK (agent <> ''),
    version TEXT NOT NULL CONSTRAINT ack_reading_version_is_named CHECK (version <> ''),
    started_at INTEGER NOT NULL,
    PRIMARY KEY (session, agent)
) STRICT, WITHOUT ROWID;

-- The tokens of a reading. Part N's token is in the last line of part N's
-- output, in the command for the next step, and part 0's token is in the
-- opening output, in the command for part 1. Fetching part N needs the token
-- of part N - 1, and the acknowledgement needs the token of the last part, so
-- an acknowledgement shows that the agent fetched every part, in order.
CREATE TABLE ack_token (
    session TEXT NOT NULL,
    agent TEXT NOT NULL,
    part INTEGER NOT NULL CONSTRAINT ack_token_part_counts_from_zero CHECK (part >= 0),
    token TEXT NOT NULL CONSTRAINT ack_token_is_named CHECK (token <> ''),
    PRIMARY KEY (session, agent, part),
    FOREIGN KEY (session, agent) REFERENCES ack_reading (session, agent) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

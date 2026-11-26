-- The agents that `builds agent run` starts and `builds agent resume` resumes.
-- agent_record.rs owns these tables and is the only code that writes them.
-- They are run records, as the cargo runs and the app builds are, and no
-- lists records: they write no lists log. Whether an agent runs now is never
-- stored: builds reads it from Claude Code's record of its running sessions
-- (claude_sessions.rs).

-- Each agent, by its Claude Code session, with the role it was started for,
-- the model and tier it runs, the folder it runs in and the brief it was
-- given.
CREATE TABLE agent (
    session TEXT PRIMARY KEY CHECK (session <> ''),
    role TEXT NOT NULL CHECK (role IN ('implementing', 'planner')),
    model TEXT NOT NULL CHECK (model <> ''),
    tier TEXT NOT NULL CHECK (tier IN ('implementation', 'strongest')),
    folder TEXT NOT NULL CHECK (folder <> ''),
    brief TEXT NOT NULL CHECK (brief <> ''),
    -- When and by whom `builds agent run` started it, seconds since 1970.
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    started_by TEXT NOT NULL CHECK (started_by <> ''),
    -- The context of the agent's last turn, in tokens, against its model's
    -- window. Both are NULL until the first turn, and the window stays NULL
    -- until the agent's result line names it.
    context_tokens INTEGER CHECK (context_tokens >= 0),
    context_window INTEGER CHECK (context_window > 0),
    CHECK ((context_window IS NULL) OR (context_tokens IS NOT NULL))
) STRICT, WITHOUT ROWID;

-- The worktree and branch of each repo an implementing agent's items run in,
-- as of its start or its last resume with an item.
CREATE TABLE agent_place (
    session TEXT NOT NULL REFERENCES agent (session) ON DELETE CASCADE,
    worktree TEXT NOT NULL CHECK (worktree <> ''),
    branch TEXT NOT NULL CHECK (branch <> ''),
    PRIMARY KEY (session, worktree)
) STRICT, WITHOUT ROWID;

-- The items an agent was started or resumed for.
CREATE TABLE agent_item (
    session TEXT NOT NULL REFERENCES agent (session) ON DELETE CASCADE,
    item INTEGER NOT NULL REFERENCES item (id),
    PRIMARY KEY (session, item)
) STRICT, WITHOUT ROWID;

-- Each process of an agent: the one `agent run` started and each one `agent
-- resume` started. `ended_at` and `exit_status` are an end that builds saw: a
-- builds that died leaves them NULL. `message` is the hash of the message
-- builds sent the process (review/texts.rs's `hash`).
CREATE TABLE agent_process (
    session TEXT NOT NULL REFERENCES agent (session) ON DELETE CASCADE,
    number INTEGER NOT NULL CHECK (number > 0),
    pid INTEGER NOT NULL CHECK (pid > 0),
    -- When the process started, seconds since 1970, which tells it from a
    -- later process that took its pid. NULL when the process had ended
    -- before the process list showed it.
    pid_started INTEGER CHECK (pid_started > 0),
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    ended_at INTEGER CHECK (ended_at >= started_at),
    exit_status INTEGER,
    message TEXT NOT NULL CHECK (message <> ''),
    CHECK ((ended_at IS NULL) = (exit_status IS NULL)),
    PRIMARY KEY (session, number)
) STRICT, WITHOUT ROWID;

-- The judge of a project's reviews (judge.rs owns this table and is the only
-- code that writes it): the standing session that settles the findings of
-- every review of the project's branches.

-- The session that last ran `builds judge listen` for the project, as its own
-- inbox and as the agent it is. A later listen replaces it. The session is the
-- project's judge while it runs. The token is that session's secret for its
-- inbox: it stays in this table, and nothing prints it, logs it, puts it on a
-- page or returns it in JSON.
CREATE TABLE review_judge (
    singleton INTEGER PRIMARY KEY CONSTRAINT review_judge_is_single CHECK (singleton = 1),
    -- The session's inbox socket (CLAUDE_CODE_MESSAGING_SOCKET).
    socket TEXT NOT NULL CONSTRAINT review_judge_socket_is_absolute CHECK (substr(socket, 1, 1) = '/'),
    -- CLAUDE_CODE_MESSAGING_TOKEN; NULL when the session had none.
    token TEXT CONSTRAINT review_judge_token_says_something CHECK (token IS NULL OR trim(token) <> ''),
    -- The session's Claude Code session id, which is also the agent id of
    -- its main agent: only a session's main agent judges.
    session TEXT NOT NULL CONSTRAINT review_judge_session_is_named CHECK (session <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_judge_by_is_named CHECK (trim(by) <> '')
) STRICT;

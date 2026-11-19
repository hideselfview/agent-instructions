-- The judge of a project's reviews, and the agent of each reviewed branch
-- (judge.rs owns these tables and is the only code that writes them): the
-- judge is the standing session that settles the findings of every review of
-- the project's branches.

-- The session that last ran `builds judge listen` for the project, as its own
-- inbox and as the agent it is. A later listen replaces it. While the session
-- runs, builds tells it of each review's findings to settle. The token is
-- that session's secret for its inbox: it stays in this table, and nothing
-- prints it, logs it, puts it on a page or returns it in JSON.
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

-- The agent that ran each review run, when a session dispatched it: the
-- agent of the branch the run read.
-- builds names an agent by its session and its agent id (ack/agent.rs). A run
-- that a session's main agent or the person ran has no row: the coordinator,
-- the judge and the person work on no branch of their own.
CREATE TABLE review_branch_agent (
    run_id INTEGER PRIMARY KEY REFERENCES review_run (id),
    session TEXT NOT NULL CONSTRAINT review_branch_agent_session_is_named CHECK (session <> ''),
    agent TEXT NOT NULL CONSTRAINT review_branch_agent_agent_is_named CHECK (agent <> ''),
    CONSTRAINT review_branch_agent_is_dispatched CHECK (agent <> session)
) STRICT;

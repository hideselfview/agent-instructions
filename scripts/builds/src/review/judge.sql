-- The judge of a project's reviews, and the agent of each reviewed branch
-- (judge.rs owns these tables and is the only code that writes them): the
-- judge is the standing session that settles the findings of every review of
-- the project's branches.

-- The session that last ran `builds judge listen` for the project, as its own
-- inbox and as the agent it is. A later listen replaces it. While the session
-- runs, builds tells it of each review's findings to settle, and only it
-- settles them. The token is
-- that session's secret for its inbox: it stays in this table, and nothing
-- prints it, logs it, puts it on a page or returns it in JSON.
CREATE TABLE review_judge (
    singleton INTEGER PRIMARY KEY CONSTRAINT review_judge_is_single CHECK (singleton = 1),
    runtime TEXT NOT NULL CONSTRAINT review_judge_runtime_known CHECK (runtime IN ('claude', 'codex')),
    address TEXT NOT NULL CONSTRAINT review_judge_address_matches_runtime CHECK (
        (runtime = 'claude' AND substr(address, 1, 1) = '/' AND instr(address, char(0)) = 0) OR
        (runtime = 'codex' AND length(CAST(address AS BLOB)) = 36
            AND substr(address, 9, 1) = '-' AND substr(address, 14, 1) = '-'
            AND substr(address, 19, 1) = '-' AND substr(address, 24, 1) = '-'
            AND length(replace(address, '-', '')) = 32
            AND replace(address, '-', '') NOT GLOB '*[^0-9a-f]*')
    ),
    token TEXT CONSTRAINT review_judge_token_is_claude_inbox_secret CHECK (token IS NULL OR (runtime = 'claude' AND trim(token) <> '')),
    -- The session's Claude Code session id, which is also the agent id of
    -- its main agent: only a session's main agent judges.
    session TEXT NOT NULL CONSTRAINT review_judge_session_is_named CHECK (session <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_judge_by_is_named CHECK (trim(by) <> ''),
    CONSTRAINT codex_judge_uses_its_own_thread CHECK (runtime = 'claude' OR session = address)
) STRICT;

-- The agent that ran each review run. The agent of the branch never settles
-- a finding it raised. A review the person ran has no row.
CREATE TABLE review_branch_agent (
    run_id INTEGER PRIMARY KEY REFERENCES review_run (id),
    session TEXT NOT NULL CONSTRAINT review_branch_agent_session_is_named CHECK (session <> ''),
    agent TEXT NOT NULL CONSTRAINT review_branch_agent_agent_is_named CHECK (agent <> '')
) STRICT;

-- A confirmed finding that someone disputed: they think the judge settled it
-- wrongly, and ask the judge once. The judge answers by settling the finding
-- again as dropped (`review_settle_again`), or by keeping it (`review_kept`).
-- A finding has one dispute: a second disagreement goes to the coordinator,
-- who asks the person whether to allow the finding to land.
CREATE TABLE review_dispute (
    finding_id INTEGER PRIMARY KEY REFERENCES review_finding (id),
    why TEXT NOT NULL CONSTRAINT review_dispute_why_says_something CHECK (trim(why) <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_dispute_by_says_something CHECK (trim(by) <> '')
) STRICT;

-- The judge's answer to a dispute that keeps the finding confirmed, and why.
CREATE TABLE review_kept (
    finding_id INTEGER PRIMARY KEY REFERENCES review_dispute (finding_id),
    why TEXT NOT NULL CONSTRAINT review_kept_why_says_something CHECK (trim(why) <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_kept_by_says_something CHECK (trim(by) <> '')
) STRICT;

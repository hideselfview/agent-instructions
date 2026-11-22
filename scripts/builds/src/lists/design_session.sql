-- The session that recorded a design change, as its own inbox: where the
-- approval is sent so that session lands the change. design.rs owns this table
-- and is the only code that writes it. The token is that session's secret for
-- its inbox: it stays in this table, and nothing prints it, logs it, puts it on
-- a page or returns it in JSON.
CREATE TABLE IF NOT EXISTS design_session (
    design_id INTEGER PRIMARY KEY REFERENCES design (id) ON DELETE CASCADE,
    runtime TEXT NOT NULL CONSTRAINT design_session_runtime_known CHECK (runtime IN ('claude', 'codex')),
    address TEXT NOT NULL CONSTRAINT design_session_address_matches_runtime CHECK (
        (runtime = 'claude' AND substr(address, 1, 1) = '/' AND instr(address, char(0)) = 0) OR
        (runtime = 'codex' AND length(CAST(address AS BLOB)) = 36
            AND substr(address, 9, 1) = '-' AND substr(address, 14, 1) = '-'
            AND substr(address, 19, 1) = '-' AND substr(address, 24, 1) = '-'
            AND length(replace(address, '-', '')) = 32
            AND replace(address, '-', '') NOT GLOB '*[^0-9a-f]*')
    ),
    token TEXT CONSTRAINT design_session_token_is_claude_inbox_secret CHECK (token IS NULL OR (runtime = 'claude' AND trim(token) <> '')),
    -- NULL before approval, then whether builds could confirm delivery.
    -- An amend clears the result.
    told TEXT CONSTRAINT design_session_told_known CHECK (told IS NULL OR told IN ('unconfirmed', 'delivered', 'queued', 'ended'))
) STRICT;

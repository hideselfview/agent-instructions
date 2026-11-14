-- The session that recorded a design change, as its own inbox: where the
-- approval is sent so that session lands the change. design.rs owns this table
-- and is the only code that writes it. The token is that session's secret for
-- its inbox: it stays in this table, and nothing prints it, logs it, puts it on
-- a page or returns it in JSON.
CREATE TABLE IF NOT EXISTS design_session (
    design_id INTEGER PRIMARY KEY REFERENCES design (id) ON DELETE CASCADE,
    -- The session's inbox socket (CLAUDE_CODE_MESSAGING_SOCKET).
    socket TEXT NOT NULL CONSTRAINT design_session_socket_is_absolute CHECK (substr(socket, 1, 1) = '/'),
    -- CLAUDE_CODE_MESSAGING_TOKEN; NULL when the session had none.
    token TEXT CONSTRAINT design_session_token_says_something CHECK (token IS NULL OR trim(token) <> ''),
    -- What the one send of the approval found: NULL before it is sent, then
    -- `delivered` (the socket took the message) or `ended` (no session listens
    -- on the socket any more). Nothing sends again; an amend clears it.
    told TEXT CONSTRAINT design_session_told_known CHECK (told IS NULL OR told IN ('delivered', 'ended'))
) STRICT;

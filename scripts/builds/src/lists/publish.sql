-- Publishing a project's designs to claude.ai from the dashboard (publish.rs
-- owns these tables and is the only code that writes them). Only a Claude
-- session can call the Artifact tool, so the dashboard's Publish button
-- records a request here and wakes the design session that listens for the
-- project; that session takes the request, publishes, and records how it
-- ended.

-- The design session that listens for the project's publish requests, as its
-- own inbox: `builds design listen` writes it, and a later listen replaces
-- it. The token is that session's secret for its inbox: it stays in this
-- table, and nothing prints it, logs it, puts it on a page or returns it in
-- JSON.
CREATE TABLE design_listener (
    singleton INTEGER PRIMARY KEY CONSTRAINT design_listener_is_single CHECK (singleton = 1),
    -- The session's inbox socket (CLAUDE_CODE_MESSAGING_SOCKET).
    socket TEXT NOT NULL CONSTRAINT design_listener_socket_is_absolute CHECK (substr(socket, 1, 1) = '/'),
    -- CLAUDE_CODE_MESSAGING_TOKEN; NULL when the session had none.
    token TEXT CONSTRAINT design_listener_token_says_something CHECK (token IS NULL OR trim(token) <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT design_listener_by_is_named CHECK (trim(by) <> '')
) STRICT;

-- A request to publish every canvas and the design system from one commit.
-- It is open until it ends, published or failed; a session takes it before it
-- publishes, and a request can fail before any session takes it (no session
-- listened on the inbox).
CREATE TABLE publish_request (
    id INTEGER PRIMARY KEY,
    -- The full id of the commit to publish.
    commit_id TEXT NOT NULL CONSTRAINT publish_request_commit_is_full CHECK (length(commit_id) = 40 AND commit_id NOT GLOB '*[^0-9a-f]*'),
    asked_at INTEGER NOT NULL,
    asked_by TEXT NOT NULL CONSTRAINT publish_request_asked_by_is_named CHECK (trim(asked_by) <> ''),
    taken_at INTEGER,
    taken_by TEXT CONSTRAINT publish_request_taken_by_is_named CHECK (taken_by IS NULL OR trim(taken_by) <> ''),
    ended_at INTEGER,
    outcome TEXT CONSTRAINT publish_request_outcome_known CHECK (outcome IS NULL OR outcome IN ('published', 'failed')),
    -- What stopped a failed request.
    error TEXT CONSTRAINT publish_request_error_says_something CHECK (error IS NULL OR trim(error) <> ''),
    CONSTRAINT publish_request_taken_whole CHECK ((taken_at IS NULL) = (taken_by IS NULL)),
    CONSTRAINT publish_request_ended_whole CHECK ((ended_at IS NULL) = (outcome IS NULL)),
    CONSTRAINT publish_request_failed_says_why CHECK ((outcome IS 'failed') = (error IS NOT NULL)),
    CONSTRAINT publish_request_published_was_taken CHECK (outcome IS NOT 'published' OR taken_at IS NOT NULL)
) STRICT;
-- At most one request is open: every open row has the same value here.
CREATE UNIQUE INDEX publish_request_one_open ON publish_request (ended_at IS NULL) WHERE ended_at IS NULL;

-- The artifacts a published request sent the designs to, as the session found
-- them on its account: `publish done` writes them with the request's end.
CREATE TABLE publish_link (
    request_id INTEGER NOT NULL REFERENCES publish_request (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CONSTRAINT publish_link_position_is_counted CHECK (position >= 0),
    title TEXT NOT NULL CONSTRAINT publish_link_title_says_something CHECK (trim(title) <> ''),
    url TEXT NOT NULL CONSTRAINT publish_link_url_is_https CHECK (url LIKE 'https://%'),
    PRIMARY KEY (request_id, position)
) STRICT;

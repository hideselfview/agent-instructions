-- Every review of a change against the rules (`builds review`): the landing it
-- belongs to, each run, each reviewer call, its verdict for every rule, and
-- each finding with how the coordinator settled it. record.rs owns these
-- tables and is the only code that writes them.

-- A landing under review: the reviews of one landing share a key until one of
-- them passes (closed_at).
CREATE TABLE review_landing (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL CONSTRAINT review_landing_key_says_something CHECK (trim(key) <> ''),
    repo TEXT NOT NULL REFERENCES repo (name),
    opened_at INTEGER NOT NULL,
    closed_at INTEGER
) STRICT;
CREATE INDEX review_landing_key ON review_landing (key);

-- One run over a diff: a landing's review, a replay of a test case, or an audit
-- of a landing that passed.
CREATE TABLE review_run (
    id INTEGER PRIMARY KEY,
    landing_id INTEGER REFERENCES review_landing (id),
    kind TEXT NOT NULL CONSTRAINT review_run_kind_known CHECK (kind IN ('review', 'replay', 'audit')),
    -- The case a replay ran, or what an audit audits.
    label TEXT,
    at INTEGER NOT NULL,
    base TEXT NOT NULL,
    head TEXT NOT NULL,
    -- A hash of the groups file the run used, so a change of grouping shows in the record.
    grouping TEXT NOT NULL,
    -- The commit of the rules checkout the run read, empty when it is no repo.
    rules_version TEXT NOT NULL,
    CONSTRAINT review_run_review_has_a_landing CHECK (kind = 'replay' OR landing_id IS NOT NULL)
) STRICT;
CREATE INDEX review_run_landing ON review_run (landing_id);

-- One reviewer process: one group, one model.
CREATE TABLE review_call (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES review_run (id),
    group_name TEXT NOT NULL,
    model TEXT NOT NULL,
    -- The rule slugs the reviewer answers for, as a JSON array.
    rules TEXT NOT NULL,
    -- All input tokens: uncached, read from the cache and written to it.
    tokens_in INTEGER NOT NULL CONSTRAINT review_call_tokens_in_not_negative CHECK (tokens_in >= 0),
    tokens_out INTEGER NOT NULL CONSTRAINT review_call_tokens_out_not_negative CHECK (tokens_out >= 0),
    -- Of tokens_in, the part read from the prompt cache and the part written to it.
    tokens_cached INTEGER NOT NULL CONSTRAINT review_call_tokens_cached_not_negative CHECK (tokens_cached >= 0),
    tokens_written INTEGER NOT NULL CONSTRAINT review_call_tokens_written_not_negative CHECK (tokens_written >= 0),
    -- For a group that lists the sites it judges: its summary and the sites, as JSON.
    sites TEXT,
    -- Why the call gave no usable answer; null when it did.
    failure TEXT
) STRICT;
CREATE INDEX review_call_run ON review_call (run_id);

-- The text a reviewer was sent and the text it answered with, kept once each
-- by its hash (an FNV-1a hash and the length).
CREATE TABLE review_text (
    hash TEXT PRIMARY KEY,
    text TEXT NOT NULL
) STRICT, WITHOUT ROWID;

-- One `claude -p` process of a call: a group asks once for each changed file
-- it reads and once for each of its passes. The system prompt and the message
-- are what it was sent; the output is its JSON answer, or why it gave none.
CREATE TABLE review_exchange (
    id INTEGER PRIMARY KEY,
    call_id INTEGER NOT NULL REFERENCES review_call (id),
    model TEXT NOT NULL,
    -- The file the exchange read, empty for a whole diff.
    unit TEXT NOT NULL,
    pass INTEGER NOT NULL,
    system_hash TEXT NOT NULL REFERENCES review_text (hash),
    message_hash TEXT NOT NULL REFERENCES review_text (hash),
    output TEXT NOT NULL,
    tokens_in INTEGER NOT NULL CONSTRAINT review_exchange_tokens_in_not_negative CHECK (tokens_in >= 0),
    tokens_out INTEGER NOT NULL CONSTRAINT review_exchange_tokens_out_not_negative CHECK (tokens_out >= 0),
    tokens_cached INTEGER NOT NULL CONSTRAINT review_exchange_tokens_cached_not_negative CHECK (tokens_cached >= 0),
    tokens_written INTEGER NOT NULL CONSTRAINT review_exchange_tokens_written_not_negative CHECK (tokens_written >= 0)
) STRICT;
CREATE INDEX review_exchange_call ON review_exchange (call_id);

CREATE TABLE review_verdict (
    call_id INTEGER NOT NULL REFERENCES review_call (id),
    rule TEXT NOT NULL,
    verdict TEXT NOT NULL CONSTRAINT review_verdict_known CHECK (verdict IN ('finding', 'none')),
    PRIMARY KEY (call_id, rule)
) STRICT, WITHOUT ROWID;

CREATE TABLE review_finding (
    id INTEGER PRIMARY KEY,
    call_id INTEGER NOT NULL REFERENCES review_call (id),
    rule TEXT NOT NULL,
    clause TEXT NOT NULL,
    file TEXT NOT NULL,
    line INTEGER NOT NULL,
    code TEXT NOT NULL,
    problem TEXT NOT NULL,
    blocking INTEGER NOT NULL CONSTRAINT review_finding_blocking_is_a_flag CHECK (blocking IN (0, 1)),
    -- Why builds dropped it: its clause is not in the rule or its code is not in the diff.
    check_failed TEXT,
    settled TEXT CONSTRAINT review_finding_settled_known CHECK (settled IN ('confirmed', 'dropped')),
    settled_why TEXT,
    settled_at INTEGER,
    settled_by TEXT,
    CONSTRAINT review_finding_settled_whole CHECK (
        (settled IS NULL AND settled_why IS NULL AND settled_at IS NULL AND settled_by IS NULL)
        OR (settled IS NOT NULL AND trim(settled_why) <> '' AND settled_at IS NOT NULL AND settled_by IS NOT NULL)
    ),
    CONSTRAINT review_finding_a_dropped_one_is_not_settled CHECK (check_failed IS NULL OR settled IS NULL)
) STRICT;
CREATE INDEX review_finding_call ON review_finding (call_id);

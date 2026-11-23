-- Every review of a change against the rules (`builds review`): the landing it
-- belongs to, each run with the net change it read, each reviewer call, its verdict
-- for every rule, each finding with how the coordinator settled it, the run
-- that closed a confirmed finding, and each confirmed finding the person
-- allowed to land. The tables also hold each copy review (`builds review
-- copy`): a run that no landing holds, with its calls and findings. record.rs
-- owns these tables and is the only code that writes them.

-- A change under review: every review of it shares this landing, found by its
-- repo and key, until the change ends. Branches of the same name in two repos
-- are two changes, each with its own landing. A landing ends in one of two
-- ways. landed_at is when `builds land` pushed the commits that left its
-- branch nothing to land (or when health found them all on origin/main), and
-- landed_run the run that covered them. abandoned_at is when health found its
-- branch and commits gone from the repo with no item keeping them, and
-- abandoned_why says what it found. An abandoned landing keeps its runs,
-- findings and settles, and never counts as landed.
CREATE TABLE review_landing (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL CONSTRAINT review_landing_key_says_something CHECK (trim(key) <> ''),
    repo TEXT NOT NULL REFERENCES repo (name),
    opened_at INTEGER NOT NULL,
    -- The last review: the run whose net change (base...head) the review has
    -- read. A review shows the changes since the last review, and nothing
    -- else. builds review writes a landing together with its first run, which
    -- it records here in the same write.
    reviewed_run INTEGER REFERENCES review_run (id),
    landed_at INTEGER,
    landed_run INTEGER REFERENCES review_run (id),
    abandoned_at INTEGER,
    abandoned_why TEXT CONSTRAINT review_landing_abandoned_says_why CHECK (abandoned_why IS NULL OR trim(abandoned_why) <> ''),
    CONSTRAINT review_landing_landed_whole CHECK ((landed_at IS NULL) = (landed_run IS NULL)),
    CONSTRAINT review_landing_abandoned_whole CHECK ((abandoned_at IS NULL) = (abandoned_why IS NULL)),
    CONSTRAINT review_landing_ends_once CHECK (landed_at IS NULL OR abandoned_at IS NULL)
) STRICT;
CREATE INDEX review_landing_change ON review_landing (repo, key);
-- A change has one landing under review at a time.
CREATE UNIQUE INDEX review_landing_open_once ON review_landing (repo, key) WHERE landed_at IS NULL AND abandoned_at IS NULL;

-- One run over a diff: a landing's review, a replay of a test case, an audit
-- of a landing that landed, or a copy review of a branch (`builds review
-- copy`), which no landing holds. A review run's base and head are the branch's net
-- change as the run found it: the common ancestor with the base it reviews
-- against, and the branch's head.
CREATE TABLE review_run (
    id INTEGER PRIMARY KEY,
    landing_id INTEGER REFERENCES review_landing (id),
    kind TEXT NOT NULL CONSTRAINT review_run_kind_known CHECK (kind IN ('review', 'replay', 'audit', 'copy')),
    -- The case a replay ran, what an audit audits, or the key of the branch a
    -- copy review read.
    label TEXT,
    at INTEGER NOT NULL,
    base TEXT NOT NULL,
    head TEXT NOT NULL,
    -- For a review run that read only how the net change from base to head
    -- differs from the net change an earlier run of its review read, that
    -- earlier run; null for a run that read the whole net change.
    since_run INTEGER REFERENCES review_run (id),
    -- The commit of the rules checkout the run read, empty when it is no repo.
    rules_version TEXT NOT NULL,
    -- How many lines the review run's diff adds and removes. The column is
    -- null for a replay, an audit or a copy review. The column is also null for a review run
    -- that builds recorded before builds counted lines, when the repo's clone
    -- on this Mac no longer held the run's commits as builds moved the store.
    changed_lines INTEGER CONSTRAINT review_run_changed_lines_not_negative CHECK (changed_lines >= 0),
    CONSTRAINT review_run_review_has_a_landing CHECK (kind IN ('replay', 'copy') OR landing_id IS NOT NULL),
    CONSTRAINT review_run_since_only_in_a_review CHECK (since_run IS NULL OR kind = 'review')
) STRICT;
CREATE INDEX review_run_landing ON review_run (landing_id);

-- One reviewer process: one group, one model.
CREATE TABLE review_call (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES review_run (id),
    group_name TEXT NOT NULL,
    model TEXT NOT NULL,
    -- The group's prompt without what each review brings (the change, the
    -- project's instructions and the brief): the system prompt's opening, the
    -- group's question, how to answer, its rules' text, its passes and the
    -- answer schema (prompt::setup), as the hash its text is kept under
    -- (texts.rs). Calls with the same model and prompt ran the same setup.
    prompt TEXT NOT NULL,
    -- The rule slugs the reviewer answers for, as a JSON array.
    rules TEXT NOT NULL,
    -- All input tokens: uncached, read from the cache and written to it.
    tokens_in INTEGER CONSTRAINT review_call_tokens_in_not_negative CHECK (tokens_in >= 0),
    tokens_out INTEGER CONSTRAINT review_call_tokens_out_not_negative CHECK (tokens_out >= 0),
    -- Of tokens_in, the part read from the prompt cache and the part written to it.
    tokens_cached INTEGER CONSTRAINT review_call_tokens_cached_not_negative CHECK (tokens_cached >= 0),
    tokens_written INTEGER CONSTRAINT review_call_tokens_written_not_negative CHECK (tokens_written >= 0),
    tokens_uncached INTEGER CONSTRAINT review_call_tokens_uncached_not_negative CHECK (tokens_uncached >= 0),
    tokens_reasoning INTEGER CONSTRAINT review_call_tokens_reasoning_not_negative CHECK (tokens_reasoning >= 0),
    -- What `claude -p` reported the call cost, in millionths of a US dollar,
    -- summed over its processes; null for a call made before builds recorded
    -- the cost.
    cost_microdollars INTEGER CONSTRAINT review_call_cost_not_negative CHECK (cost_microdollars >= 0),
    -- For a group that lists the sites it judges: its summary and the sites, as JSON.
    sites TEXT,
    -- Why the call gave no usable answer; null when it did.
    failure TEXT
) STRICT;
CREATE INDEX review_call_run ON review_call (run_id);

-- One `claude -p` process of a call: a group asks once for each changed file
-- it reads and once for each of its passes. The system prompt and the message
-- are what it was sent; the output is its JSON answer, or why it gave none.
-- The three are kept as the hashes of their texts, which builds keeps on the
-- build volume for a while (texts.rs).
CREATE TABLE review_exchange (
    id INTEGER PRIMARY KEY,
    call_id INTEGER NOT NULL REFERENCES review_call (id),
    model TEXT NOT NULL,
    -- The file the exchange read, empty for a whole diff.
    unit TEXT NOT NULL,
    pass INTEGER NOT NULL,
    system_hash TEXT NOT NULL,
    message_hash TEXT NOT NULL,
    output_hash TEXT NOT NULL,
    tokens_in INTEGER CONSTRAINT review_exchange_tokens_in_not_negative CHECK (tokens_in >= 0),
    tokens_out INTEGER CONSTRAINT review_exchange_tokens_out_not_negative CHECK (tokens_out >= 0),
    tokens_cached INTEGER CONSTRAINT review_exchange_tokens_cached_not_negative CHECK (tokens_cached >= 0),
    tokens_written INTEGER CONSTRAINT review_exchange_tokens_written_not_negative CHECK (tokens_written >= 0),
    tokens_uncached INTEGER CONSTRAINT review_exchange_tokens_uncached_not_negative CHECK (tokens_uncached >= 0),
    tokens_reasoning INTEGER CONSTRAINT review_exchange_tokens_reasoning_not_negative CHECK (tokens_reasoning >= 0)
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
    -- Why builds dropped it: its clause is not in the rule, it is on the
    -- commit messages and its group reads none, its code is on no line the
    -- diff adds or removes, or its code sits only on comment lines and its
    -- rule's group judges no code comment.
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

-- A second settle: the coordinator changed an open confirmed finding to
-- dropped. review_finding keeps the first settle, and this row keeps the
-- second, which is how the finding counts from then on.
CREATE TABLE review_settle_again (
    finding_id INTEGER PRIMARY KEY REFERENCES review_finding (id),
    why TEXT NOT NULL CONSTRAINT review_settle_again_why_says_something CHECK (trim(why) <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_settle_again_by_says_something CHECK (trim(by) <> '')
) STRICT;

-- A confirmed finding that a later review run closed: the changes that run
-- read fix it. builds gives each review the open confirmed findings of the
-- groups it asks, and each reviewer names the ones the changes fix. A
-- confirmed finding that no run closed, that the coordinator did not settle
-- again as dropped and that the person did not allow to land is open, and
-- `builds land` refuses the branch while one is.
CREATE TABLE review_closed (
    finding_id INTEGER PRIMARY KEY REFERENCES review_finding (id),
    run_id INTEGER NOT NULL REFERENCES review_run (id)
) STRICT;

-- An open confirmed finding that the person allowed to land: the finding is
-- true, and its code lands as it is. why holds the person's words, and by
-- names who recorded them.
CREATE TABLE review_allowed (
    finding_id INTEGER PRIMARY KEY REFERENCES review_finding (id),
    why TEXT NOT NULL CONSTRAINT review_allowed_why_says_something CHECK (trim(why) <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_allowed_by_says_something CHECK (trim(by) <> '')
) STRICT;

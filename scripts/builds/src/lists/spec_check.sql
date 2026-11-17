-- Each check of a brief against a repo's product spec, which `builds item
-- take` runs before items start with the brief: the items, what the reviewer
-- was sent and answered, and each conflict and unstated behavior it found.
-- spec_check.rs owns these tables and is the only code that writes them.

-- One reviewer process, run in one worktree the take names, over the spec
-- files of that worktree's repo.
CREATE TABLE spec_check (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT spec_check_by_says_something CHECK (trim(by) <> ''),
    -- The brief as the take named it: a path, then `#section` when it names one.
    brief TEXT NOT NULL CONSTRAINT spec_check_brief_says_something CHECK (trim(brief) <> ''),
    -- The brief file's text as the reviewer read it; plans are in no commit,
    -- so only this row keeps it.
    brief_text TEXT NOT NULL,
    repo TEXT NOT NULL REFERENCES repo (name),
    worktree TEXT NOT NULL CONSTRAINT spec_check_worktree_is_absolute CHECK (substr(worktree, 1, 1) = '/'),
    -- The commit the worktree's HEAD was at when the reviewer read its spec files.
    head TEXT NOT NULL CONSTRAINT spec_check_head_is_a_commit CHECK (length(head) = 40 AND head NOT GLOB '*[^0-9a-f]*'),
    -- The spec files the reviewer read, as a JSON array of paths in the repo.
    spec_files TEXT NOT NULL,
    model TEXT NOT NULL,
    -- The commit of the rules checkout the check read, empty when it is no repo.
    rules_version TEXT NOT NULL,
    system TEXT NOT NULL,
    message TEXT NOT NULL,
    -- The reviewer's JSON answer, or why it gave none.
    output TEXT NOT NULL,
    tokens_in INTEGER NOT NULL CONSTRAINT spec_check_tokens_in_not_negative CHECK (tokens_in >= 0),
    tokens_out INTEGER NOT NULL CONSTRAINT spec_check_tokens_out_not_negative CHECK (tokens_out >= 0),
    tokens_cached INTEGER NOT NULL CONSTRAINT spec_check_tokens_cached_not_negative CHECK (tokens_cached >= 0),
    tokens_written INTEGER NOT NULL CONSTRAINT spec_check_tokens_written_not_negative CHECK (tokens_written >= 0),
    cost_microdollars INTEGER NOT NULL CONSTRAINT spec_check_cost_not_negative CHECK (cost_microdollars >= 0),
    -- Why the check gave no usable answer; null when it did.
    failure TEXT,
    -- The decision or question that records the person's choice on the
    -- conflicts, which the take named with --spec-decided.
    decided_id INTEGER,
    decided_kind TEXT CONSTRAINT spec_check_decided_by_a_decision_or_question CHECK (decided_kind IN ('decision', 'question')),
    CONSTRAINT spec_check_decided_whole CHECK ((decided_id IS NULL) = (decided_kind IS NULL)),
    CONSTRAINT spec_check_decided_names_its_record FOREIGN KEY (decided_id, decided_kind) REFERENCES record (id, kind)
) STRICT;

-- The items the take took, or meant to take, with the checked brief.
CREATE TABLE spec_check_item (
    check_id INTEGER NOT NULL REFERENCES spec_check (id),
    item_id INTEGER NOT NULL REFERENCES item (id),
    PRIMARY KEY (check_id, item_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX spec_check_item_item ON spec_check_item (item_id);

-- What a check found: a sentence of the brief that conflicts with the spec's
-- text, which the finding quotes with its file and line, or a sentence that
-- states a behavior the spec doesn't state.
CREATE TABLE spec_check_finding (
    id INTEGER PRIMARY KEY,
    check_id INTEGER NOT NULL REFERENCES spec_check (id),
    kind TEXT NOT NULL CONSTRAINT spec_check_finding_kind_known CHECK (kind IN ('conflict', 'unstated')),
    -- The brief's sentence, as the reviewer quoted it.
    brief_quote TEXT NOT NULL,
    spec_file TEXT,
    -- The line of spec_file where builds found spec_quote; null when builds
    -- found no such text there.
    spec_line INTEGER CONSTRAINT spec_check_finding_line_positive CHECK (spec_line >= 1),
    spec_quote TEXT,
    why TEXT NOT NULL,
    -- Why builds set the finding aside: a quote that is not in the brief or
    -- in the spec file it names.
    check_failed TEXT,
    CONSTRAINT spec_check_finding_a_conflict_quotes_the_spec CHECK (
        (kind = 'conflict') = (spec_file IS NOT NULL AND spec_quote IS NOT NULL)
    ),
    CONSTRAINT spec_check_finding_a_kept_conflict_has_its_line CHECK (
        (kind = 'conflict' AND check_failed IS NULL) = (spec_line IS NOT NULL)
    )
) STRICT;
CREATE INDEX spec_check_finding_check ON spec_check_finding (check_id);

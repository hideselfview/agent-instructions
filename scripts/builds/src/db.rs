//! `builds.sqlite`, the one database in each project's store folder: the
//! lists (every table of `lists/*.sql`), the hook-run stats (`hook_stats.sql`
//! and `hook_check_starts.sql`), the app-build timings (`app_timing.sql`), the
//! design changes' recording sessions (`lists/design_session.sql`), the
//! review's tables, the `builds cargo` runs
//! (`cargo_run.sql`), the spec checks (`lists/spec_check.sql`), the agents'
//! acknowledgements (`ack/record.sql`), the judge (`review/judge.sql`). This module does all opening:
//! the pragmas, the schema made whole in the first transaction of a new file,
//! and the stamp.
//!
//! The stamp is `PRAGMA user_version`, a hash of the shape of the assembled
//! schema text: the text without its comments and spacing (`sql_shape`). A
//! database whose stamp differs was made by another builds schema and is
//! refused; there is no version number and no chain of changes. The schema
//! and its stamp are constants the compiler computes, and migrate.rs refuses
//! to compile a stamp it does not record, so a change to the schema comes with
//! the move from the schema before it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::fail;
use crate::fail::{Context, Result};

/// The database's file name in a project's store folder.
pub const DATABASE: &str = "builds.sqlite";

const HOOK_PART: &str = include_str!("hook_stats.sql");

/// The tags of questions and items (lists/tag.sql), read after the records.
pub const TAG_PART: &str = include_str!("lists/tag.sql");
/// The app-build timing tables (app_timing.sql), the last part of the schema.
pub const APP_TIMING_PART: &str = include_str!("app_timing.sql");

/// The design changes' recording sessions (lists/design_session.sql), the last
/// part of the schema.
pub const DESIGN_SESSION_PART: &str = include_str!("lists/design_session.sql");

/// A busy store waits as `flock` did: until the other write ends. SQLite
/// takes the wait as milliseconds in an `i32`.
const BUSY: Duration = Duration::from_millis(i32::MAX as u64);

/// The review's tables (review/record.sql).
pub const REVIEW_PART: &str = include_str!("review/record.sql");

/// This Mac's load when each hook check started (hook_check_starts.sql).
pub const HOOK_CHECK_STARTS_PART: &str = include_str!("hook_check_starts.sql");

/// The record of the `builds cargo` runs hook checks started (cargo_run.sql),
/// the last part of the schema.
pub const CARGO_RUNS_PART: &str = include_str!("cargo_run.sql");

/// Where open items keep a worktree with no agent (lists/kept_work.sql); the
/// schema holds it after the cargo runs' part.
pub const KEPT_WORK_PART: &str = include_str!("lists/kept_work.sql");

/// The work an item held when the person dropped it (lists/dropped.sql); the
/// schema holds it after the kept work's part.
pub const DROPPED_PART: &str = include_str!("lists/dropped.sql");

/// The checks of briefs against the product spec (lists/spec_check.sql); the
/// schema holds it after the dropped items' part.
pub const SPEC_CHECK_PART: &str = include_str!("lists/spec_check.sql");

/// The agents' acknowledgements of the text builds asks them to read
/// (ack/record.sql); the schema holds it after the spec checks' part.
pub const ACK_PART: &str = include_str!("ack/record.sql");

/// The design session that listens for publish requests, and the requests
/// to publish the designs (lists/publish.sql); the schema holds it after the
/// acknowledgements' part.
pub const PUBLISH_PART: &str = include_str!("lists/publish.sql");

/// The item made for each stable Rust release (lists/rust_release.sql); the
/// schema holds it after the publish requests' part.
pub const RUST_RELEASE_PART: &str = include_str!("lists/rust_release.sql");

/// How the last check for a stable Rust release ended (lists/rust_check.sql);
/// the schema holds it after the Rust release items' part.
pub const RUST_CHECK_PART: &str = include_str!("lists/rust_check.sql");

/// The moments an item changed state or gained or lost a wait
/// (lists/item_event.sql); the schema holds it after the Rust checks' part.
pub const ITEM_EVENT_PART: &str = include_str!("lists/item_event.sql");

/// The record of every `builds land` run (land_run.sql); the schema holds it
/// after the items' events' part.
pub const LAND_RUN_PART: &str = include_str!("land_run.sql");

/// When each reviewer process of a review began and how long it ran
/// (review/exchange_time.sql); the schema holds it after the landings' part.
pub const EXCHANGE_TIME_PART: &str = include_str!("review/exchange_time.sql");

/// What each build host said about its load and memory over time
/// (host_sample.sql); the schema holds it after the reviewer processes' times.
pub const HOST_SAMPLE_PART: &str = include_str!("host_sample.sql");

/// The crates each `builds cargo` run named (cargo_run_crate.sql); the schema
/// holds it after the hosts' samples.
pub const CARGO_RUN_CRATE_PART: &str = include_str!("cargo_run_crate.sql");

/// The judge of the project's reviews (review/judge.sql); the schema holds
/// it after the crates of the cargo runs.
pub const JUDGE_PART: &str = include_str!("review/judge.sql");

/// Each change of a worktree card's dashboard state (card_state.sql); the
/// schema holds it after the judge's tables.
pub const CARD_STATE_PART: &str = include_str!("card_state.sql");

/// How each `builds cargo` run ended (cargo_run_outcome.sql); the schema holds
/// it after the cards' states.
pub const CARGO_RUN_OUTCOME_PART: &str = include_str!("cargo_run_outcome.sql");

/// The commit each `builds cargo` run ran on (cargo_run_commit.sql); the
/// schema holds it after the cargo runs' outcomes.
pub const CARGO_RUN_COMMIT_PART: &str = include_str!("cargo_run_commit.sql");

/// The commit each app build run built (app_build_commit.sql); the schema
/// holds it after the cargo runs' commits.
pub const APP_BUILD_COMMIT_PART: &str = include_str!("app_build_commit.sql");

/// What each failed hook check said (hook_check_failure.sql); the schema
/// holds it after the commits of the cargo runs and the app builds.
pub const HOOK_CHECK_FAILURE_PART: &str = include_str!("hook_check_failure.sql");

/// What each failed app build step said (app_build_step_failure.sql); the
/// schema holds it after what the failed hook checks said.
pub const APP_BUILD_STEP_FAILURE_PART: &str = include_str!("app_build_step_failure.sql");

/// What builds told the judge of the findings that waited for it
/// (review/judge_told.sql); the schema holds it after what the failed app
/// build steps said.
pub const JUDGE_TOLD_PART: &str = include_str!("review/judge_told.sql");

/// The circles of a review board that surround a box
/// (lists/design_circle_box.sql); the schema holds it after what builds told
/// the judge.
pub const DESIGN_CIRCLE_BOX_PART: &str = include_str!("lists/design_circle_box.sql");

/// The branches over the review's cap (review/over_cap.sql); the schema holds
/// it after the circles of a review board that surround a box.
pub const OVER_CAP_PART: &str = include_str!("review/over_cap.sql");

/// The branches the coordinator marked as mechanical work
/// (review/marked_mechanical.sql); the schema holds it after the branches over
/// the review's cap.
pub const MARKED_MECHANICAL_PART: &str = include_str!("review/marked_mechanical.sql");

/// The shots prefixes an agent dropped from a branch's landing
/// (app_shots_drop.sql); the schema holds it after the branches marked as
/// mechanical work.
pub const APP_SHOTS_DROP_PART: &str = include_str!("app_shots_drop.sql");

/// What each shots step of an app build used (app_build_step_shots.sql); the
/// schema holds it after the shots prefixes dropped from a landing.
pub const APP_BUILD_STEP_SHOTS_PART: &str = include_str!("app_build_step_shots.sql");

/// Each part of the schema, in the order the schema holds them: the lists'
/// tables, then the hook stats', then the app-build timings', then the design
/// changes' recording sessions', then the review's, then the hook checks'
/// starting loads, then the cargo runs', then
/// the kept work's, then the dropped items' work, then the spec checks', then
/// the acknowledgements', then the publish requests', then the Rust release
/// items', then the Rust checks', then the items' events, then the landings',
/// then the reviewer processes' times, then the hosts' samples, then the
/// crates of the cargo runs, then the judge's, then the cards' states, then
/// the cargo runs' outcomes, then the commits of the cargo runs and the app
/// builds, then what the failed hook checks said, then what the failed app
/// build steps said, then what builds told the judge, then the circles of a
/// review board that surround a box, then the branches over the review's
/// cap. A table points at ones earlier or
/// later (SQLite resolves foreign keys when rows are written), but the order is
/// the one to read the schema in. A part that joins the schema goes at the end.
const SCHEMA_PARTS: [&str; 42] = [
    include_str!("lists/project.sql"),
    include_str!("lists/area.sql"),
    include_str!("lists/record.sql"),
    TAG_PART,
    include_str!("lists/question.sql"),
    include_str!("lists/decision.sql"),
    include_str!("lists/item.sql"),
    include_str!("lists/running.sql"),
    include_str!("lists/landing.sql"),
    include_str!("lists/design.sql"),
    include_str!("lists/store.sql"),
    HOOK_PART,
    APP_TIMING_PART,
    DESIGN_SESSION_PART,
    REVIEW_PART,
    HOOK_CHECK_STARTS_PART,
    CARGO_RUNS_PART,
    KEPT_WORK_PART,
    DROPPED_PART,
    SPEC_CHECK_PART,
    ACK_PART,
    PUBLISH_PART,
    RUST_RELEASE_PART,
    RUST_CHECK_PART,
    ITEM_EVENT_PART,
    LAND_RUN_PART,
    EXCHANGE_TIME_PART,
    HOST_SAMPLE_PART,
    CARGO_RUN_CRATE_PART,
    JUDGE_PART,
    CARD_STATE_PART,
    CARGO_RUN_OUTCOME_PART,
    CARGO_RUN_COMMIT_PART,
    APP_BUILD_COMMIT_PART,
    HOOK_CHECK_FAILURE_PART,
    APP_BUILD_STEP_FAILURE_PART,
    JUDGE_TOLD_PART,
    DESIGN_CIRCLE_BOX_PART,
    OVER_CAP_PART,
    MARKED_MECHANICAL_PART,
    APP_SHOTS_DROP_PART,
    APP_BUILD_STEP_SHOTS_PART,
];

/// The length of <parts> joined with a newline between each two.
const fn joined_len(parts: &[&str]) -> usize {
    let mut len = 0;
    let mut at = 0;
    while at < parts.len() {
        len += parts[at].len();
        at += 1;
    }
    len + parts.len() - 1
}

/// <parts> joined with a newline between each two, as the bytes of a text
/// <N> bytes long (`joined_len`).
const fn joined<const N: usize>(parts: &[&str]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut written = 0;
    let mut part = 0;
    while part < parts.len() {
        if part > 0 {
            out[written] = b'\n';
            written += 1;
        }
        let bytes = parts[part].as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            out[written] = bytes[at];
            written += 1;
            at += 1;
        }
        part += 1;
    }
    assert!(written == N, "the joined parts fill the text");
    out
}

static SCHEMA_BYTES: [u8; joined_len(&SCHEMA_PARTS)] = joined(&SCHEMA_PARTS);

/// The whole schema: each part of `SCHEMA_PARTS`, in order, with a newline
/// between each two.
pub const SCHEMA: &str = match std::str::from_utf8(&SCHEMA_BYTES) {
    Ok(text) => text,
    Err(_) => panic!("the schema's parts are text, so their join is too"),
};

/// The stamp of today's schema, computed while the program compiles.
pub const STAMP: i64 = stamp_of_bytes(SCHEMA.as_bytes());

/// The whole schema (`SCHEMA`).
pub fn schema() -> String {
    SCHEMA.to_string()
}

/// The table of the `--shots` runs' findings, which every schema held
/// between the design changes' recording sessions and the review's tables
/// until builds kept the runs on the build volume (shots_cache.rs).
#[cfg(test)]
pub const SHOT_FINDINGS_TABLE: &str = "CREATE TABLE IF NOT EXISTS shot_findings (
    repo TEXT NOT NULL CHECK (repo <> ''),
    commit_id TEXT NOT NULL CHECK (commit_id <> ''),
    platform TEXT NOT NULL CHECK (platform IN ('mac', 'android')),
    prefix TEXT NOT NULL CHECK (prefix <> ''),
    shots_version TEXT NOT NULL CHECK (shots_version <> ''),
    recorded_at INTEGER NOT NULL CHECK (recorded_at > 0),
    exit_code INTEGER NOT NULL CHECK (exit_code >= 0),
    findings TEXT NOT NULL,
    PRIMARY KEY (repo, commit_id, platform, prefix, shots_version)
) STRICT, WITHOUT ROWID;";

/// <schema> as it was before builds kept the shots runs on the build volume:
/// with the shot findings' table before the review's tables.
#[cfg(test)]
pub fn with_shot_findings(schema: String) -> String {
    match schema.find("CREATE TABLE review_landing (") {
        Some(at) => format!("{}{SHOT_FINDINGS_TABLE}\n{}", &schema[..at], &schema[at..]),
        None => format!("{schema}\n{SHOT_FINDINGS_TABLE}"),
    }
}

/// The table of the review's texts, which every schema held between the
/// review's calls and exchanges until builds kept the texts on the build volume
/// (review/texts.rs).
#[cfg(test)]
const REVIEW_TEXT_TABLE: &str = "CREATE TABLE review_text (
    hash TEXT PRIMARY KEY,
    text TEXT NOT NULL
) STRICT;
";

/// <schema> as it was before builds kept the review texts on the build
/// volume: with the texts' table, the references to it, and each exchange's
/// answer in place of its hash. A schema with no review tables is returned as
/// it is.
#[cfg(test)]
pub fn with_review_texts(schema: String) -> String {
    const EXCHANGE: &str = "CREATE TABLE review_exchange (";
    const HASHES: &str = "    system_hash TEXT NOT NULL,\n    message_hash TEXT NOT NULL,\n    output_hash TEXT NOT NULL,\n";
    const PROMPT: &str = "    prompt TEXT NOT NULL,\n";
    if !schema.contains(EXCHANGE) {
        return schema;
    }
    assert_eq!(schema.matches(HASHES).count(), 1);
    assert_eq!(schema.matches(PROMPT).count(), 1);
    schema
        .replacen(
            HASHES,
            "    system_hash TEXT NOT NULL REFERENCES review_text (hash),\n    message_hash TEXT NOT NULL REFERENCES review_text (hash),\n    output TEXT NOT NULL,\n",
            1,
        )
        .replacen(
            PROMPT,
            "    prompt TEXT NOT NULL REFERENCES review_text (hash),\n",
            1,
        )
        .replacen(EXCHANGE, &format!("{REVIEW_TEXT_TABLE}{EXCHANGE}"), 1)
}

/// The acknowledgements' table as it was before nothing read the time of an
/// acknowledgement, with its comment.
#[cfg(test)]
const ACK_TABLE_WITH_TIME: &str =
    "-- Each agent's last acknowledgement of the text: the version of the text it
-- read and when.
CREATE TABLE ack (
    session TEXT NOT NULL CONSTRAINT ack_session_is_named CHECK (session <> ''),
    agent TEXT NOT NULL CONSTRAINT ack_agent_is_named CHECK (agent <> ''),
    version TEXT NOT NULL CONSTRAINT ack_version_is_named CHECK (version <> ''),
    at INTEGER NOT NULL,
    PRIMARY KEY (session, agent)
) STRICT, WITHOUT ROWID;";

/// The table of the sections a reading held, with its comment, as it was
/// before every reading held the whole text.
#[cfg(test)]
const ACK_READING_SECTION_TABLE: &str = "-- The sections a reading holds, by name, in the order its parts show them:
-- every section of the text, or only those that changed since the version
-- of that text the agent last acknowledged.
CREATE TABLE ack_reading_section (
    session TEXT NOT NULL,
    agent TEXT NOT NULL,
    position INTEGER NOT NULL CONSTRAINT ack_reading_section_position_counts_from_one CHECK (position >= 1),
    name TEXT NOT NULL CONSTRAINT ack_reading_section_name_is_named CHECK (name <> ''),
    PRIMARY KEY (session, agent, position),
    UNIQUE (session, agent, name),
    FOREIGN KEY (session, agent) REFERENCES ack_reading (session, agent) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

";

/// The review page's entry table as it was before an entry could be a
/// component's preview: every entry is a board with a theme.
#[cfg(test)]
const REVIEW_BOARD_ONLY_BOARDS: &str = "CREATE TABLE review_board (
    design_id INTEGER NOT NULL,
    position INTEGER NOT NULL CONSTRAINT review_board_position_not_negative CHECK (position >= 0),
    board TEXT NOT NULL CONSTRAINT review_board_is_a_board_file CHECK (board NOT GLOB '*#*' AND board NOT GLOB '*/'),
    caption TEXT NOT NULL CONSTRAINT review_board_caption_says_something CHECK (trim(caption) <> ''),
    theme TEXT NOT NULL CONSTRAINT review_board_theme_known CHECK (theme IN ('light', 'dark')),
    show_now INTEGER NOT NULL CONSTRAINT review_board_show_now_is_a_flag CHECK (show_now IN (0, 1)),
    PRIMARY KEY (design_id, position),
    CONSTRAINT review_board_is_on_a_section FOREIGN KEY (design_id) REFERENCES review_section (design_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;";

/// The comment and table of the deleted places as the schema had them when a
/// change could delete only boards.
#[cfg(test)]
const DELETED_BOARDS_ONLY: &str = "-- A board the change deletes: a board place of its range that is no file at the
-- range's last commit. It needs no answer, canvas link or place on the review
-- page, which names it as deleted.
CREATE TABLE design_deleted (
    design_id INTEGER NOT NULL REFERENCES design (id) ON DELETE CASCADE,
    path TEXT NOT NULL CONSTRAINT design_deleted_path_is_a_board_file CHECK (path NOT GLOB '*#*' AND path NOT GLOB '*/'),
    PRIMARY KEY (design_id, path)
) STRICT, WITHOUT ROWID;";

/// <schema> without the table of what the shots steps used, the last part of
/// the schema.
#[cfg(test)]
fn without_step_shots(schema: String) -> String {
    without_unreported_usage(schema.replacen(&format!("\n{APP_BUILD_STEP_SHOTS_PART}"), "", 1))
}

#[cfg(test)]
fn without_unreported_usage(mut schema: String) -> String {
    for table in ["review_call", "review_exchange", "spec_check"] {
        for counter in ["in", "out", "cached", "written"] {
            let start = format!("tokens_{counter} INTEGER");
            let constraint = format!(" CONSTRAINT {table}_tokens_{counter}_not_negative");
            schema = schema.replace(
                &format!("{start}{constraint}"),
                &format!("{start} NOT NULL{constraint}"),
            );
        }
        for counter in ["uncached", "reasoning"] {
            let column = format!(
                ",\n    tokens_{counter} INTEGER CONSTRAINT {table}_tokens_{counter}_not_negative CHECK (tokens_{counter} >= 0)"
            );
            schema = schema.replace(&column, "");
        }
    }
    schema.replacen(
        "cost_microdollars INTEGER CONSTRAINT spec_check_cost_not_negative",
        "cost_microdollars INTEGER NOT NULL CONSTRAINT spec_check_cost_not_negative",
        1,
    )
}

/// Derives uncached input from the counters that legacy stores kept.
pub const LEGACY_UNCACHED_INPUT: &str = "CASE WHEN tokens_in - tokens_cached >= tokens_written THEN tokens_in - tokens_cached - tokens_written END";

/// Assignments that remove all legacy counters when the reviewer reported none.
pub const CLEAR_LEGACY_TOKENS: &str = "tokens_in = NULL, tokens_out = NULL, tokens_cached = NULL, tokens_written = NULL, tokens_uncached = NULL";

/// The schema before review records could omit usage and spec-check cost.
#[cfg(test)]
pub fn schema_before_unreported_usage() -> String {
    without_unreported_usage(schema_before_codex_inboxes())
}

/// <schema> as it was before a change could delete a component: with the
/// deleted places' table refusing a folder, and without what the shots steps
/// used (`schema_before_app_build_step_shots`). A schema with no such table
/// is returned as it is. Every older schema than today's goes through this
/// function.
#[cfg(test)]
pub fn with_deleted_boards_only(schema: String) -> String {
    let schema = without_at_merge_base(without_step_shots(schema));
    const START: &str = "-- A board or component the change deletes";
    if !schema.contains(START) {
        return schema;
    }
    let table = format!(
        "{START}{}",
        statement(&schema, START, ") STRICT, WITHOUT ROWID;")
            .strip_prefix(START)
            .expect("it starts with START")
    );
    schema.replacen(&table, DELETED_BOARDS_ONLY, 1)
}

/// <schema> as it was before the app build steps' table kept whether a step
/// ran at the merge base: without its `at_merge_base` column.
#[cfg(test)]
pub fn without_at_merge_base(schema: String) -> String {
    schema.replacen(
        "    at_merge_base INTEGER CHECK (at_merge_base IN (0, 1)),\n",
        "",
        1,
    )
}

/// The inbox tables before a destination could name a Codex thread.
#[cfg(test)]
fn with_claude_inboxes(mut schema: String) -> String {
    const DESIGN_SESSION: &str = r#"CREATE TABLE IF NOT EXISTS design_session (
    design_id INTEGER PRIMARY KEY REFERENCES design (id) ON DELETE CASCADE,
    -- The session's inbox socket (CLAUDE_CODE_MESSAGING_SOCKET).
    socket TEXT NOT NULL CONSTRAINT design_session_socket_is_absolute CHECK (substr(socket, 1, 1) = '/'),
    -- CLAUDE_CODE_MESSAGING_TOKEN; NULL when the session had none.
    token TEXT CONSTRAINT design_session_token_says_something CHECK (token IS NULL OR trim(token) <> ''),
    -- What the one send of the approval found: NULL before it is sent, then
    -- `delivered` (the socket took the message) or `ended` (no session listens
    -- on the socket any more). Nothing sends again; an amend clears it.
    told TEXT CONSTRAINT design_session_told_known CHECK (told IS NULL OR told IN ('delivered', 'ended'))
) STRICT;"#;
    const DESIGN_LISTENER: &str = r#"CREATE TABLE design_listener (
    singleton INTEGER PRIMARY KEY CONSTRAINT design_listener_is_single CHECK (singleton = 1),
    -- The session's inbox socket (CLAUDE_CODE_MESSAGING_SOCKET).
    socket TEXT NOT NULL CONSTRAINT design_listener_socket_is_absolute CHECK (substr(socket, 1, 1) = '/'),
    -- CLAUDE_CODE_MESSAGING_TOKEN; NULL when the session had none.
    token TEXT CONSTRAINT design_listener_token_says_something CHECK (token IS NULL OR trim(token) <> ''),
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT design_listener_by_is_named CHECK (trim(by) <> '')
) STRICT;"#;
    const REVIEW_JUDGE: &str = r#"CREATE TABLE review_judge (
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
) STRICT;"#;
    for (start, old) in [
        (
            "CREATE TABLE IF NOT EXISTS design_session (",
            DESIGN_SESSION,
        ),
        ("CREATE TABLE design_listener (", DESIGN_LISTENER),
        (
            "CREATE TABLE publish_request (",
            crate::lists::publish::PUBLISH_REQUEST,
        ),
        ("CREATE TABLE review_judge (", REVIEW_JUDGE),
        (
            "CREATE TABLE review_branch_agent (",
            crate::review::judge::REVIEW_BRANCH_AGENT,
        ),
    ] {
        if schema.contains(start) {
            let table = statement(&schema, start, ") STRICT;").to_string();
            schema = schema.replacen(&table, old, 1);
        }
    }
    schema
}

#[cfg(test)]
pub fn schema_before_codex_inboxes() -> String {
    with_claude_inboxes(SCHEMA.to_string())
}

/// The schema as it was before the app build steps' table kept whether a step
/// ran at the merge base.
#[cfg(test)]
pub fn schema_before_app_build_at_merge_base() -> String {
    without_at_merge_base(without_step_shots(schema_before_codex_inboxes()))
}

/// The schema as it was before a change could delete a component.
#[cfg(test)]
pub fn schema_before_deleted_place() -> String {
    with_deleted_boards_only(schema_before_codex_inboxes())
}

/// <schema> as it was before a review page's entry could be a component's
/// preview. A schema with no such table is returned as it is. It also puts
/// back the deleted places' boards-only table (`with_deleted_boards_only`),
/// so each older schema has it.
#[cfg(test)]
pub fn with_review_boards_only(schema: String) -> String {
    let schema = with_deleted_boards_only(schema);
    const START: &str = "CREATE TABLE review_board (";
    if !schema.contains(START) {
        return schema;
    }
    let table = statement(&schema, START, ") STRICT, WITHOUT ROWID;").to_string();
    schema.replacen(&table, REVIEW_BOARD_ONLY_BOARDS, 1)
}

/// The schema as it was before a review page's entry could be a component's
/// preview.
#[cfg(test)]
pub fn schema_before_review_component() -> String {
    with_review_boards_only(schema_before_codex_inboxes())
}

/// <schema> as it was before nothing read the time of an acknowledgement or
/// the sections of a reading: with the acknowledgements' table keeping the
/// time and the table of a reading's sections. A schema with no
/// acknowledgements' table is returned as it is. Every older schema than
/// today's goes through this function, so it also puts back the review
/// page's boards-only entry table (`with_review_boards_only`).
#[cfg(test)]
pub fn with_ack_unread(schema: String) -> String {
    let schema = with_review_boards_only(schema);
    const START: &str = "-- Each agent's last acknowledgement of the text: the version of the text it\n-- read.\nCREATE TABLE ack (";
    const TOKENS: &str = "-- The tokens of a reading.";
    if !schema.contains(START) {
        return schema;
    }
    let table = format!(
        "{}{}",
        "-- Each agent's last acknowledgement of the text: the version of the text it\n-- read.\n",
        statement(&schema, "CREATE TABLE ack (", ") STRICT, WITHOUT ROWID;")
    );
    schema.replacen(&table, ACK_TABLE_WITH_TIME, 1).replacen(
        TOKENS,
        &format!("{ACK_READING_SECTION_TABLE}{TOKENS}"),
        1,
    )
}

/// The schema as it was before nothing read the time of an acknowledgement or
/// the sections of a reading.
#[cfg(test)]
pub fn schema_before_ack_unread() -> String {
    with_ack_unread(schema_before_codex_inboxes())
}

/// The table of a reading's made-up words as it was before a reading's parts
/// were chained by tokens.
#[cfg(test)]
const ACK_WORD_TABLE: &str = "CREATE TABLE ack_word (
    session TEXT NOT NULL,
    agent TEXT NOT NULL,
    part INTEGER NOT NULL CONSTRAINT ack_word_part_counts_from_one CHECK (part >= 1),
    position INTEGER NOT NULL CONSTRAINT ack_word_position_counts_from_one CHECK (position >= 1),
    word TEXT NOT NULL CONSTRAINT ack_word_is_a_word CHECK (word <> '' AND word NOT GLOB '*[^a-z]*'),
    PRIMARY KEY (session, agent, part, position),
    FOREIGN KEY (session, agent) REFERENCES ack_reading (session, agent) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;";

/// <schema> as it was before a reading's parts were chained by tokens: with
/// the made-up words' table in place of the tokens' table. A schema with no
/// such table is returned as it is.
#[cfg(test)]
pub fn with_ack_words(schema: String) -> String {
    let schema = with_ack_unread(schema);
    const START: &str = "CREATE TABLE ack_token (";
    if !schema.contains(START) {
        return schema;
    }
    let table = statement(&schema, START, ") STRICT, WITHOUT ROWID;").to_string();
    schema.replacen(&table, ACK_WORD_TABLE, 1)
}

/// The schema as it was before a reading's parts were chained by tokens.
#[cfg(test)]
pub fn schema_before_ack_tokens() -> String {
    with_ack_words(schema_before_codex_inboxes())
}

/// The table of the agents' acknowledgements as it was before the text left
/// out the CLAUDE.md files: keyed by the repo whose CLAUDE.md the text held,
/// with an index for the text of each repo and one for the text of none.
#[cfg(test)]
const ACK_TABLE_WITH_REPO: &str = "CREATE TABLE ack (
    session TEXT NOT NULL CONSTRAINT ack_session_is_named CHECK (session <> ''),
    agent TEXT NOT NULL CONSTRAINT ack_agent_is_named CHECK (agent <> ''),
    repo TEXT CONSTRAINT ack_repo_is_named CHECK (repo <> ''),
    version TEXT NOT NULL CONSTRAINT ack_version_is_named CHECK (version <> ''),
    at INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX ack_of_repo ON ack (session, agent, repo) WHERE repo IS NOT NULL;
CREATE UNIQUE INDEX ack_of_no_repo ON ack (session, agent) WHERE repo IS NULL;";

/// <schema> as it was before the text left out the CLAUDE.md files: with the
/// acknowledgements' table keyed by repo, and the made-up words' table (which
/// that schema held). A schema with no such table is returned as it is.
#[cfg(test)]
pub fn with_ack_repo(schema: String) -> String {
    let schema = with_ack_words(schema);
    const START: &str = "CREATE TABLE ack (";
    if !schema.contains(START) {
        return schema;
    }
    let table = statement(&schema, START, ") STRICT, WITHOUT ROWID;").to_string();
    schema.replacen(&table, ACK_TABLE_WITH_REPO, 1)
}

/// The schema as it was before the text left out the CLAUDE.md files: every
/// part, with the acknowledgements' table keyed by repo.
#[cfg(test)]
pub fn schema_before_ack_repo() -> String {
    with_ack_repo(schema_before_codex_inboxes())
}

/// The schema as it was before <part> joined it: the parts of
/// `SCHEMA_PARTS` before <part>, with a newline between each two, and the
/// shot findings' table and the review texts' table and references, which
/// every such schema held.
#[cfg(test)]
fn schema_before(part: &str) -> String {
    let at = SCHEMA_PARTS
        .iter()
        .position(|known| *known == part)
        .expect("the part is one of the schema's parts");
    with_ack_repo(with_review_texts(with_shot_findings(with_claude_inboxes(
        SCHEMA_PARTS[..at].join("\n"),
    ))))
}

/// The schema as it was before what the shots steps used joined it: every
/// part but the last.
#[cfg(test)]
pub fn schema_before_app_build_step_shots() -> String {
    without_step_shots(schema_before_codex_inboxes())
}

/// The schema as it was before the shots prefixes dropped from a landing
/// joined it: every part but the last two, with none of the older schemas'
/// differences put back.
#[cfg(test)]
pub fn schema_before_app_shots_drop() -> String {
    with_ack_repo(with_claude_inboxes(
        SCHEMA_PARTS[..SCHEMA_PARTS.len() - 2].join("\n"),
    ))
}

/// The schema as it was before the branches the coordinator marked as
/// mechanical joined it: every part but the last three, with none of the
/// older schemas' differences put back.
#[cfg(test)]
pub fn schema_before_marked_mechanical() -> String {
    with_ack_repo(with_claude_inboxes(
        SCHEMA_PARTS[..SCHEMA_PARTS.len() - 3].join("\n"),
    ))
}

/// The schema as it was before the branches over the review's cap joined it,
/// less the variants of a design change, which it held after the circles
/// that surround a box and which migrate.rs's tests put back. This and each
/// older schema below hold today's text of the review section: before the
/// section kept only its heading, each kept the review page's words too,
/// which migrate.rs's tests put back.
#[cfg(test)]
pub fn schema_before_over_cap() -> String {
    schema_before(OVER_CAP_PART)
}

/// The schema as it was before the variants of a design change joined it:
/// the parts before the branches over the review's cap, which joined after
/// the variants.
#[cfg(test)]
pub fn schema_before_design_variants() -> String {
    schema_before(OVER_CAP_PART)
}

/// The schema as it was before the boxes' circles joined it.
#[cfg(test)]
pub fn schema_before_design_circle_box() -> String {
    schema_before(DESIGN_CIRCLE_BOX_PART)
}

/// The schema as it was before what builds told the judge joined it.
#[cfg(test)]
pub fn schema_before_judge_told() -> String {
    schema_before(JUDGE_TOLD_PART)
}

/// The schema as it was before what the failed app build steps said joined
/// it.
#[cfg(test)]
pub fn schema_before_app_build_step_failure() -> String {
    schema_before(APP_BUILD_STEP_FAILURE_PART)
}

/// The schema as it was before what the failed hook checks said joined it.
#[cfg(test)]
pub fn schema_before_hook_check_failure() -> String {
    schema_before(HOOK_CHECK_FAILURE_PART)
}

/// The schema as it was before the commits of the cargo runs and the app
/// builds joined it.
#[cfg(test)]
pub fn schema_before_step_commits() -> String {
    schema_before(CARGO_RUN_COMMIT_PART)
}

/// The schema as it was before the cargo runs' outcomes joined it.
#[cfg(test)]
pub fn schema_before_cargo_run_outcome() -> String {
    schema_before(CARGO_RUN_OUTCOME_PART)
}

/// The schema as it was before the cards' states joined it.
#[cfg(test)]
pub fn schema_before_card_state() -> String {
    schema_before(CARD_STATE_PART)
}

/// The schema as it was before the judge's tables joined it. This and each
/// older schema below hold today's text of the parts they share with today's
/// schema: before builds found a review's landing by its repo and key, each
/// had an index on the landing's key alone, which migrate.rs's tests put
/// back.
#[cfg(test)]
pub fn schema_before_judge() -> String {
    schema_before(JUDGE_PART)
}

/// The schema as it was before the cargo runs' crates joined it.
#[cfg(test)]
pub fn schema_before_cargo_run_crate() -> String {
    schema_before(CARGO_RUN_CRATE_PART)
}

/// The schema as it was before the hosts' samples joined it.
#[cfg(test)]
pub fn schema_before_host_sample() -> String {
    schema_before(HOST_SAMPLE_PART)
}

/// The schema as it was before the reviewer processes' times joined it.
#[cfg(test)]
pub fn schema_before_exchange_time() -> String {
    schema_before(EXCHANGE_TIME_PART)
}

/// The schema as it was before the landings' tables joined it.
#[cfg(test)]
pub fn schema_before_land_run() -> String {
    schema_before(LAND_RUN_PART)
}

/// The schema as it was before the items' events joined it.
#[cfg(test)]
pub fn schema_before_item_event() -> String {
    schema_before(ITEM_EVENT_PART)
}

/// The statement of <schema> that starts with <from>, through the first <end>
/// after it. A move that makes a table again takes the table's statements
/// from the schema text this way, so the table equals a fresh database's.
pub fn statement<'a>(schema: &'a str, from: &str, end: &str) -> &'a str {
    let Some(start) = schema.find(from) else {
        panic!("the schema has no statement that starts with {from:?}");
    };
    let Some(length) = schema[start..].find(end) else {
        panic!("the statement that starts with {from:?} has no {end:?} after it");
    };
    &schema[start..start + length + end.len()]
}

/// Makes table <table> again as <create> writes it, for a store move: renames
/// the old table to `<table>_old`, runs <create>, lets <copy> copy the old
/// rows from `<table>_old` into the new table, drops the old table and runs
/// <after> (the table's indexes and any table the move adds beside it).
/// migrate.rs runs a move with the foreign keys off, and SQLite renames the
/// old table under `legacy_alter_table`, so the tables that point at <table>
/// keep naming it.
pub fn remake_table(
    db: &Connection,
    table: &str,
    create: &str,
    copy: impl FnOnce(&Connection) -> Result<()>,
    after: &[&str],
) -> Result<()> {
    db.execute_batch(&format!(
        "PRAGMA legacy_alter_table = ON;
ALTER TABLE {table} RENAME TO {table}_old;
{create}"
    ))
    .sql()?;
    copy(db)?;
    db.execute_batch(&format!(
        "DROP TABLE {table}_old;
{}
PRAGMA legacy_alter_table = OFF;",
        after.join("\n")
    ))
    .sql()
}

/// The stamp of today's schema (`STAMP`).
pub fn stamp() -> i64 {
    STAMP
}

/// The stamp of a schema whose text is <text> (`stamp_of_bytes`).
#[cfg(test)]
pub fn stamp_of_text(text: &str) -> i64 {
    stamp_of_bytes(text.as_bytes())
}

/// The stamp of a schema whose text's bytes are <sql>: the FNV-1a hash
/// (`text::fnv1a_add`) of the text's shape (`sql_shape::next`), so a comment
/// or a change of spacing leaves the stamp as it was, folded into
/// `user_version`'s positive range and kept off 0 (no schema) and 1 (which a
/// hook-stats-only database once carried).
const fn stamp_of_bytes(sql: &[u8]) -> i64 {
    let mut hash = crate::text::FNV1A_START;
    let mut reader = crate::sql_shape::Reader::START;
    while let Some((byte, after)) = crate::sql_shape::next(sql, reader) {
        hash = crate::text::fnv1a_add(hash, byte);
        reader = after;
    }
    (hash % (i32::MAX as u32 - 2)) as i64 + 2
}

fn configure(conn: &Connection, path: &Path) -> Result<()> {
    let what = || format!("configuring {}", path.display());
    conn.busy_timeout(BUSY).context(what)?;
    conn.pragma_update(None, "foreign_keys", true)
        .context(what)?;
    conn.pragma_update(None, "synchronous", "FULL")
        .context(what)
}

/// The stamp a file holds: 0 for none.
pub fn stamp_of(conn: &Connection, path: &Path) -> Result<i64> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .context(|| format!("reading {}", path.display()))
}

/// The stamp the database at <path> holds, read without the stamp check and
/// without a use of the stores: 0 for none. `builds install` reads it to
/// learn whether it moves the store.
pub fn stamp_at(path: &Path) -> Result<i64> {
    stamp_of(&connect_read_only(path)?, path)
}

/// Refuses a stamp that is not this schema's, naming both.
fn require_stamp(found: i64, path: &Path) -> Result<()> {
    if found != stamp() && crate::install_gate::install_ran()? {
        fail!(
            "builds install ran while this command ran, and moved {} to the installed program's schema; run the command again",
            path.display()
        );
    }
    if found != stamp() {
        fail!(
            "{} was made by another builds schema (it carries {found}, this builds' schema is {}); `builds install` moves a store of an older one",
            path.display(),
            stamp()
        );
    }
    Ok(())
}

fn has_tables(conn: &Connection, path: &Path) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .context(|| format!("reading {}", path.display()))?;
    Ok(count > 0)
}

/// How a writer may find the file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Make {
    /// A file that is not there is made, with the whole schema.
    Allowed,
    /// A file that is not there is an error.
    Never,
}

/// A connection to a store, and the use of the stores it takes while it is
/// open (install_gate.rs).
pub struct Held {
    // Rust drops fields in the order they are declared, so `conn` closes
    // before `_use` ends the use.
    conn: Connection,
    _use: Option<crate::install_gate::ConnectionUse>,
}

impl std::ops::Deref for Held {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.conn.fmt(f)
    }
}

impl std::ops::DerefMut for Held {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

/// The database at <path> for writing, with the schema's stamp. The folder
/// must exist; this never makes it. A new file gets the whole schema and the
/// stamp in its first transaction, in WAL mode.
pub fn open(path: &Path, make: Make) -> Result<Held> {
    let taken = crate::install_gate::for_connection()?;
    let conn = open_connection(path, make)?;
    Ok(Held { conn, _use: taken })
}

fn open_connection(path: &Path, make: Make) -> Result<Connection> {
    let flags = match make {
        Make::Allowed => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        Make::Never => OpenFlags::SQLITE_OPEN_READ_WRITE,
    };
    let mut conn = Connection::open_with_flags(path, flags)
        .context(|| format!("opening {}", path.display()))?;
    configure(&conn, path)?;
    let found = stamp_of(&conn, path)?;
    if found == 0 {
        if has_tables(&conn, path)? {
            fail!(
                "{} holds tables and no schema stamp; it is no builds.sqlite of this builds",
                path.display()
            );
        }
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
            .context(|| format!("configuring {}", path.display()))?;
        let what = || format!("making the schema in {}", path.display());
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context(what)?;
        // Another opener may have made it while this one waited.
        if stamp_of(&tx, path)? == 0 {
            tx.execute_batch(&schema()).context(what)?;
            tx.pragma_update(None, "user_version", stamp())
                .context(what)?;
        }
        tx.commit().context(what)?;
        return Ok(conn);
    }
    require_stamp(found, path)?;
    Ok(conn)
}

/// The database at <path> for reading, read-only; None when there is no file
/// or no schema in it yet.
pub fn open_reader(path: &Path) -> Result<Option<Held>> {
    if !path.exists() {
        return Ok(None);
    }
    let taken = crate::install_gate::for_connection()?;
    Ok(open_read_only(path)?.map(|conn| Held { conn, _use: taken }))
}

/// What tells a reader that runs for long, such as the daemon, that the
/// database at a path changed: a read-only connection it holds open, whose
/// `data_version` moves with every commit another connection makes, and the
/// file's identity, size and modification time, which move when something
/// outside SQLite rewrites or replaces the file. The held connection reads no
/// table and checks no schema stamp. While it is open, the reader's other
/// connections never open as the database's only one, which is when SQLite
/// resets the shared-memory file and writes into the database's folder.
pub struct Watch {
    path: PathBuf,
    conn: Connection,
    file: FileMark,
    data_version: i64,
}

/// A file's device, inode, size and modification time.
#[derive(PartialEq, Eq)]
struct FileMark(u64, u64, u64, i64, i64);

impl FileMark {
    fn of(path: &Path) -> Result<FileMark> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).context(|| format!("reading {}", path.display()))?;
        Ok(FileMark(
            meta.dev(),
            meta.ino(),
            meta.size(),
            meta.mtime(),
            meta.mtime_nsec(),
        ))
    }
}

impl Watch {
    /// Starts watching the database at <path>.
    pub fn open(path: &Path) -> Result<Watch> {
        let conn = connect_read_only(path)?;
        let data_version = data_version(&conn, path)?;
        Ok(Watch {
            path: path.to_path_buf(),
            file: FileMark::of(path)?,
            conn,
            data_version,
        })
    }

    /// Whether the database changed since the watch opened or since the last
    /// call that said so. Fails when the file or the connection can't be
    /// read, which leaves whether it changed unknown.
    pub fn moved(&mut self) -> Result<bool> {
        let file = FileMark::of(&self.path)?;
        let data_version = data_version(&self.conn, &self.path)?;
        let moved = file != self.file || data_version != self.data_version;
        self.file = file;
        self.data_version = data_version;
        Ok(moved)
    }
}

/// The connection's `data_version`, which moves each time another connection
/// commits to the database.
fn data_version(conn: &Connection, path: &Path) -> Result<i64> {
    conn.pragma_query_value(None, "data_version", |row| row.get(0))
        .context(|| format!("reading {}", path.display()))
}

/// A read-only connection to the database at <path> that waits while
/// another connection writes, with no stamp check.
fn connect_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context(|| format!("opening {}", path.display()))?;
    conn.busy_timeout(BUSY)
        .context(|| format!("configuring {}", path.display()))?;
    Ok(conn)
}

fn open_read_only(path: &Path) -> Result<Option<Connection>> {
    let conn = connect_read_only(path)?;
    let found = stamp_of(&conn, path)?;
    if found == 0 {
        return Ok(None);
    }
    require_stamp(found, path)?;
    Ok(Some(conn))
}

/// A new in-memory database with the whole schema: the same pragmas, nothing
/// on disk. Tests build their lists in one.
pub fn memory() -> Connection {
    let conn = Connection::open_in_memory().expect("an in-memory database opens");
    conn.pragma_update(None, "foreign_keys", true)
        .expect("pragma");
    conn.execute_batch(&schema()).expect("the schema applies");
    conn.pragma_update(None, "user_version", stamp())
        .expect("pragma");
    conn
}

/// The schema as the database holds it: every table and index, by name, with
/// the shape of its SQL (`sql_shape`).
pub fn schema_rows(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut statement = conn
        .prepare("SELECT type, name, COALESCE(sql, '') FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name")
        .context(|| "reading the schema".to_string())?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .context(|| "reading the schema".to_string())?;
    let rows: Vec<(String, String, String)> = rows
        .collect::<std::result::Result<_, _>>()
        .context(|| "reading the schema".to_string())?;
    Ok(rows
        .into_iter()
        .map(|(kind, name, sql)| (kind, name, crate::sql_shape::shape(&sql)))
        .collect())
}

/// Names a refused or failed statement on the lists' database.
pub trait Sql<T> {
    fn sql(self) -> Result<T>;
}

impl<T> Sql<T> for rusqlite::Result<T> {
    fn sql(self) -> Result<T> {
        self.map_err(|error| {
            crate::fail::Fail::Message(format!("builds.sqlite refused the write: {error}"))
        })
    }
}

/// Writes one row of <table>, which must be one of <owned>, with the values
/// given and every constraint holding: the planting functions of the owners
/// that need a row a constraint refuses.
#[cfg(test)]
pub fn plant_row(
    db: &Connection,
    owned: &[&str],
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    assert!(
        owned.contains(&table),
        "{table} is not one of this owner's tables ({owned:?})"
    );
    let names: Vec<&str> = columns.iter().map(|(name, _)| *name).collect();
    let marks: Vec<String> = (1..=columns.len()).map(|n| format!("?{n}")).collect();
    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        names.join(", "),
        marks.join(", ")
    );
    db.execute(
        &sql,
        rusqlite::params_from_iter(columns.iter().map(|(_, value)| value)),
    )
}

/// The names of the constraints a part of the schema gives, in its text.
#[cfg(test)]
pub fn constraint_names(sql: &str) -> Vec<String> {
    let words: Vec<&str> = sql
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    let mut names = Vec::new();
    for pair in words.windows(2) {
        if pair[0] == "CONSTRAINT" {
            names.push(pair[1].to_string());
        }
    }
    names
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;

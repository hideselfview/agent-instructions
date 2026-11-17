//! What reviews wrote down, in the tables of `record.sql`: this file is the
//! only code that writes them. A change under review keeps one landing, found
//! by its key, until it lands, and names the run of its last review; a run
//! holds the net change it read, the run of the last review when it read only
//! the changes since that run, and one call per group; a call holds a verdict
//! for every rule and the findings; the coordinator settles a finding as confirmed or dropped, and
//! may settle an open confirmed finding again, as dropped. A later review run
//! closes a confirmed finding when the changes it read fix it.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;

/// A finding a reviewer raised, as it goes into the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFinding {
    pub rule: String,
    pub clause: String,
    pub file: String,
    pub line: i64,
    pub code: String,
    pub problem: String,
    pub blocking: bool,
    /// Why builds dropped it before the coordinator saw it.
    pub check_failed: Option<String>,
}

/// One `claude -p` process of a call, with everything it was sent and
/// answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewExchange {
    pub model: String,
    /// The file it read, empty for a whole diff.
    pub unit: String,
    pub pass: i64,
    pub system: String,
    pub message: String,
    /// The reviewer's JSON answer, or why it gave none.
    pub output: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_cached: i64,
    pub tokens_written: i64,
}

/// One reviewer call, as it goes into the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCall {
    pub group: String,
    pub model: String,
    /// The group's prompt without what each review brings (`prompt::setup`).
    pub prompt: String,
    pub rules: Vec<String>,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_cached: i64,
    pub tokens_written: i64,
    /// What `claude -p` reported the call's processes cost, in millionths of
    /// a US dollar.
    pub cost_microdollars: i64,
    /// The sites a group that lists them judged, as JSON.
    pub sites: Option<String>,
    pub failure: Option<String>,
    /// (rule, `finding` or `none`).
    pub verdicts: Vec<(String, String)>,
    pub findings: Vec<NewFinding>,
    /// The open confirmed findings the reviewer found the changes fix.
    pub closes: Vec<i64>,
    /// Every process the call ran.
    pub exchanges: Vec<NewExchange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Review,
    Replay,
    Audit,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Review => "review",
            Kind::Replay => "replay",
            Kind::Audit => "audit",
        }
    }
}

/// A finding as the store holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub id: i64,
    pub run_id: i64,
    /// `review`, `replay` or `audit`.
    pub kind: String,
    pub landing_id: Option<i64>,
    pub group: String,
    pub rule: String,
    pub clause: String,
    pub file: String,
    pub line: i64,
    pub code: String,
    pub problem: String,
    pub blocking: bool,
    pub check_failed: Option<String>,
    /// How the coordinator settled it, once or twice.
    pub settled: Option<Settled>,
    /// The review run that closed it, for a confirmed finding the changes that
    /// run read fix.
    pub closed_by: Option<i64>,
}

/// The settles of a finding: one, or a first settle and the second that
/// changed it to dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settled {
    Once(Settle),
    Again { first: Settle, second: Settle },
}

impl Settled {
    /// The settle the finding counts as: the second, when there is one.
    pub fn last(&self) -> &Settle {
        match self {
            Settled::Once(settle) => settle,
            Settled::Again { second, .. } => second,
        }
    }
}

/// How the coordinator settled a finding, and who did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settle {
    /// `confirmed` or `dropped`.
    pub how: String,
    pub why: String,
    pub by: String,
}

impl Finding {
    /// Whether builds' own checks accepted it: a finding the coordinator sees.
    pub fn counts(&self) -> bool {
        self.check_failed.is_none()
    }

    /// Whether the coordinator settled it as <how>.
    pub fn settled_as(&self, how: &str) -> bool {
        self.settled
            .as_ref()
            .is_some_and(|settled| settled.last().how == how)
    }

    /// Whether it is an open confirmed finding of a review: confirmed, and no
    /// review run closed it.
    pub fn is_open(&self) -> bool {
        self.kind == "review"
            && self.counts()
            && self.settled_as("confirmed")
            && self.closed_by.is_none()
    }
}

/// A call as the store holds it, without its findings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub id: i64,
    pub run_id: i64,
    pub kind: String,
    pub group: String,
    pub model: String,
    /// The hash its prompt is kept under in `review_text`.
    pub prompt: String,
    /// The rule slugs it answers for.
    pub rules: Vec<String>,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_cached: i64,
    pub tokens_written: i64,
    /// None for a call made before builds recorded the cost.
    pub cost_microdollars: Option<i64>,
    pub failure: Option<String>,
}

/// A rule's verdict in a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub call_id: i64,
    pub rule: String,
    /// `finding` or `none`.
    pub verdict: String,
}

/// The landing of <key> whose change has not landed, if any.
pub fn open_landing(db: &Connection, key: &str) -> Result<Option<i64>> {
    db.query_row(
        "SELECT id FROM review_landing WHERE key = ?1 AND landed_at IS NULL ORDER BY id DESC LIMIT 1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .sql()
}

pub fn add_landing(db: &Connection, key: &str, repo: &str, now: i64) -> Result<i64> {
    db.execute(
        "INSERT INTO review_landing (key, repo, opened_at) VALUES (?1, ?2, ?3)",
        params![key, repo, now],
    )
    .sql()?;
    Ok(db.last_insert_rowid())
}

/// Records <run> as the last review of <landing>: the next review shows the
/// changes since the net change <run> read. record_reviewed refuses a
/// landing whose change landed.
pub fn record_reviewed(db: &Connection, landing: i64, run: i64) -> Result<()> {
    let changed = db
        .execute(
            "UPDATE review_landing SET reviewed_run = ?2 WHERE id = ?1 AND landed_at IS NULL",
            params![landing, run],
        )
        .sql()?;
    if changed == 0 {
        fail!("review landing {landing} is not open: it landed already or does not exist");
    }
    Ok(())
}

/// Records that <landing>'s change landed, with <run> the run that covered
/// the commits pushed. record_landed refuses a landing that landed already.
pub fn record_landed(db: &Connection, landing: i64, run: i64, now: i64) -> Result<()> {
    let changed = db
        .execute(
            "UPDATE review_landing SET landed_at = ?2, landed_run = ?3 WHERE id = ?1 AND landed_at IS NULL",
            params![landing, now, run],
        )
        .sql()?;
    if changed == 0 {
        fail!("review landing {landing} is not open: it landed already or does not exist");
    }
    Ok(())
}

/// A run over a diff, as it goes into the store.
pub struct NewRun<'a> {
    pub landing: Option<i64>,
    pub kind: Kind,
    /// What a replay replays or an audit audits.
    pub label: Option<&'a str>,
    pub at: i64,
    pub base: &'a str,
    pub head: &'a str,
    /// For a review run that reads only the changes since the last review,
    /// the run of that last review.
    pub since: Option<i64>,
    /// A hash of the groups' settings and rules the run used.
    pub grouping: &'a str,
    /// The commit of the rules checkout the run read.
    pub rules_version: &'a str,
}

/// The hash <text> is kept under: its FNV-1a hash and its length.
pub fn text_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}-{}", text.len())
}

/// Keeps <text> once under its hash and returns the hash.
fn keep_text(db: &Connection, text: &str) -> Result<String> {
    let hash = text_hash(text);
    db.execute(
        "INSERT OR IGNORE INTO review_text (hash, text) VALUES (?1, ?2)",
        params![hash, text],
    )
    .sql()?;
    Ok(hash)
}

/// Writes a run with its calls, verdicts and findings, and closes each
/// finding a call closes. Returns the run's id and, per call in order, the
/// ids its findings got.
pub fn add_run(db: &Connection, run: &NewRun, calls: &[NewCall]) -> Result<(i64, Vec<Vec<i64>>)> {
    db.execute(
        "INSERT INTO review_run (landing_id, kind, label, at, base, head, since_run, grouping, rules_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![run.landing, run.kind.word(), run.label, run.at, run.base, run.head, run.since, run.grouping, run.rules_version],
    )
    .sql()?;
    let run_id = db.last_insert_rowid();
    let mut ids = Vec::new();
    for call in calls {
        let rules = serde_json::to_string(&call.rules).expect("strings serialize");
        let prompt = keep_text(db, &call.prompt)?;
        db.execute(
            "INSERT INTO review_call (run_id, group_name, model, prompt, rules, tokens_in, tokens_out, tokens_cached, tokens_written, cost_microdollars, sites, failure) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![run_id, call.group, call.model, prompt, rules, call.tokens_in, call.tokens_out, call.tokens_cached, call.tokens_written, call.cost_microdollars, call.sites, call.failure],
        )
        .sql()?;
        let call_id = db.last_insert_rowid();
        for (rule, verdict) in &call.verdicts {
            db.execute(
                "INSERT INTO review_verdict (call_id, rule, verdict) VALUES (?1, ?2, ?3)",
                params![call_id, rule, verdict],
            )
            .sql()?;
        }
        for exchange in &call.exchanges {
            let system_hash = keep_text(db, &exchange.system)?;
            let message_hash = keep_text(db, &exchange.message)?;
            db.execute(
                "INSERT INTO review_exchange (call_id, model, unit, pass, system_hash, message_hash, output, tokens_in, tokens_out, tokens_cached, tokens_written) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![call_id, exchange.model, exchange.unit, exchange.pass, system_hash, message_hash, exchange.output, exchange.tokens_in, exchange.tokens_out, exchange.tokens_cached, exchange.tokens_written],
            )
            .sql()?;
        }
        let mut finding_ids = Vec::new();
        for f in &call.findings {
            db.execute(
                "INSERT INTO review_finding (call_id, rule, clause, file, line, code, problem, blocking, check_failed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![call_id, f.rule, f.clause, f.file, f.line, f.code, f.problem, f.blocking, f.check_failed],
            )
            .sql()?;
            finding_ids.push(db.last_insert_rowid());
        }
        ids.push(finding_ids);
        for finding in &call.closes {
            close(db, *finding, run_id, run.landing)?;
        }
    }
    Ok((run_id, ids))
}

/// Closes open confirmed finding <id> of <landing>: run <run> read changes
/// that fix it. close refuses a finding of another landing and one that is
/// not open.
fn close(db: &Connection, id: i64, run: i64, landing: Option<i64>) -> Result<()> {
    let finding = finding_of(db, id)?;
    if finding.landing_id != landing || landing.is_none() {
        fail!("finding {id} is not a finding of the landing run {run} reviews");
    }
    if !finding.is_open() {
        fail!("finding {id} is not an open confirmed finding, so no review closes it");
    }
    db.execute(
        "INSERT INTO review_closed (finding_id, run_id) VALUES (?1, ?2)",
        params![id, run],
    )
    .sql()?;
    Ok(())
}

/// Settles finding <id> as `confirmed` or `dropped`. A finding builds dropped
/// by its own checks, or one settled already, is refused.
pub fn settle(db: &Connection, id: i64, how: &str, why: &str, now: i64, by: &str) -> Result<()> {
    if !["confirmed", "dropped"].contains(&how) {
        fail!("a finding is settled as confirmed or dropped, not {how}");
    }
    if why.trim().is_empty() {
        fail!("--why says nothing");
    }
    let row: Option<(Option<String>, Option<String>)> = db
        .query_row(
            "SELECT check_failed, settled FROM review_finding WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .sql()?;
    let Some((check_failed, settled)) = row else {
        fail!("there is no finding {id}");
    };
    if let Some(reason) = check_failed {
        fail!("builds' own check dropped finding {id} ({reason}); there is nothing to settle");
    }
    if let Some(settled) = settled {
        fail!(
            "finding {id} is settled already, as {settled}. To change an open confirmed finding to dropped, settle it again: builds review settle {id} --again --why TEXT"
        );
    }
    db.execute(
        "UPDATE review_finding SET settled = ?2, settled_why = ?3, settled_at = ?4, settled_by = ?5 WHERE id = ?1",
        params![id, how, why, now, by],
    )
    .sql()?;
    Ok(())
}

/// Settles finding <id> again, as dropped: only an open confirmed finding
/// changes. The finding keeps its first settle beside the second,
/// and counts as the second from then on.
pub fn settle_again(db: &Connection, id: i64, why: &str, now: i64, by: &str) -> Result<()> {
    if why.trim().is_empty() {
        fail!("--why says nothing");
    }
    let finding = finding_of(db, id)?;
    if let Some(reason) = &finding.check_failed {
        fail!("builds' own check dropped finding {id} ({reason}); there is nothing to settle");
    }
    let settled = match &finding.settled {
        None => fail!(
            "finding {id} is not settled; settle it first: builds review settle {id} confirmed|dropped --why TEXT"
        ),
        Some(Settled::Again { .. }) => {
            fail!("finding {id} is settled again already, as dropped")
        }
        Some(Settled::Once(settled)) => settled,
    };
    if settled.how != "confirmed" {
        fail!("finding {id} is settled as {} already", settled.how);
    }
    if let Some(run) = finding.closed_by {
        fail!(
            "review run {run} closed finding {id}, so it stays confirmed: builds refuses to settle a closed finding again"
        );
    }
    db.execute(
        "INSERT INTO review_settle_again (finding_id, why, at, by) VALUES (?1, ?2, ?3, ?4)",
        params![id, why, now, by],
    )
    .sql()?;
    Ok(())
}

const FINDING_COLUMNS: &str = "f.id, c.run_id, r.kind, r.landing_id, c.group_name, f.rule, f.clause, f.file, f.line, f.code, f.problem, f.blocking, f.check_failed, f.settled, f.settled_why, f.settled_by, a.why, a.by, z.run_id";

fn finding_from(row: &rusqlite::Row) -> rusqlite::Result<Finding> {
    let settled: Option<String> = row.get(13)?;
    let why: Option<String> = row.get(14)?;
    let by: Option<String> = row.get(15)?;
    let again_why: Option<String> = row.get(16)?;
    let again_by: Option<String> = row.get(17)?;
    let first = match (settled, why, by) {
        (Some(how), Some(why), Some(by)) => Some(Settle { how, why, by }),
        _ => None,
    };
    let again = again_why.zip(again_by).map(|(why, by)| Settle {
        how: "dropped".to_string(),
        why,
        by,
    });
    let settled = match (first, again) {
        (Some(first), Some(second)) => Some(Settled::Again { first, second }),
        (Some(first), None) => Some(Settled::Once(first)),
        (None, _) => None,
    };
    Ok(Finding {
        id: row.get(0)?,
        run_id: row.get(1)?,
        kind: row.get(2)?,
        landing_id: row.get(3)?,
        group: row.get(4)?,
        rule: row.get(5)?,
        clause: row.get(6)?,
        file: row.get(7)?,
        line: row.get(8)?,
        code: row.get(9)?,
        problem: row.get(10)?,
        blocking: row.get(11)?,
        check_failed: row.get(12)?,
        settled,
        closed_by: row.get(18)?,
    })
}

const FINDING_FROM: &str = "FROM review_finding f JOIN review_call c ON c.id = f.call_id JOIN review_run r ON r.id = c.run_id LEFT JOIN review_settle_again a ON a.finding_id = f.id LEFT JOIN review_closed z ON z.finding_id = f.id";

/// Every finding of <run>, in the order raised.
pub fn findings_of_run(db: &Connection, run: i64) -> Result<Vec<Finding>> {
    let sql = format!("SELECT {FINDING_COLUMNS} {FINDING_FROM} WHERE c.run_id = ?1 ORDER BY f.id");
    let mut statement = db.prepare(&sql).sql()?;
    let rows = statement.query_map(params![run], finding_from).sql()?;
    rows.map(|row| row.sql()).collect()
}

/// Every finding of every run of <landing>, in the order raised.
pub fn findings_of_landing(db: &Connection, landing: i64) -> Result<Vec<Finding>> {
    let sql =
        format!("SELECT {FINDING_COLUMNS} {FINDING_FROM} WHERE r.landing_id = ?1 ORDER BY f.id");
    let mut statement = db.prepare(&sql).sql()?;
    let rows = statement.query_map(params![landing], finding_from).sql()?;
    rows.map(|row| row.sql()).collect()
}

/// Finding <id> as the store holds it.
pub fn finding_of(db: &Connection, id: i64) -> Result<Finding> {
    let sql = format!("SELECT {FINDING_COLUMNS} {FINDING_FROM} WHERE f.id = ?1");
    db.query_row(&sql, params![id], finding_from)
        .optional()
        .sql()?
        .ok_or_else(|| crate::fail::Fail::Message(format!("there is no finding {id}")))
}

/// The calls of <landing>'s runs, in the order made.
pub fn calls_of_landing(db: &Connection, landing: i64) -> Result<Vec<Call>> {
    calls_where(db, "r.landing_id = ?1", landing)
}

/// The calls of <run>.
pub fn calls_of_run(db: &Connection, run: i64) -> Result<Vec<Call>> {
    calls_where(db, "c.run_id = ?1", run)
}

/// The calls of every run of <kind> (`review`, `replay` or `audit`).
pub fn calls_of_kind(db: &Connection, kind: Kind) -> Result<Vec<Call>> {
    calls_where(db, "r.kind = ?1", kind.word())
}

fn calls_where(db: &Connection, condition: &str, value: impl rusqlite::ToSql) -> Result<Vec<Call>> {
    let sql = format!(
        "SELECT c.id, c.run_id, r.kind, c.group_name, c.model, c.prompt, c.rules, c.tokens_in, c.tokens_out, c.tokens_cached, c.tokens_written, c.cost_microdollars, c.failure FROM review_call c JOIN review_run r ON r.id = c.run_id WHERE {condition} ORDER BY c.id"
    );
    let mut statement = db.prepare(&sql).sql()?;
    let rows = statement
        .query_map(params![value], |row| {
            let rules: String = row.get(6)?;
            Ok(Call {
                id: row.get(0)?,
                run_id: row.get(1)?,
                kind: row.get(2)?,
                group: row.get(3)?,
                model: row.get(4)?,
                prompt: row.get(5)?,
                rules: serde_json::from_str(&rules).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                tokens_in: row.get(7)?,
                tokens_out: row.get(8)?,
                tokens_cached: row.get(9)?,
                tokens_written: row.get(10)?,
                cost_microdollars: row.get(11)?,
                failure: row.get(12)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// The verdicts of the calls of <landing>'s runs.
pub fn verdicts_of_landing(db: &Connection, landing: i64) -> Result<Vec<Verdict>> {
    verdicts_where(db, "r.landing_id = ?1", landing)
}

/// The verdicts of the calls of <run>.
pub fn verdicts_of_run(db: &Connection, run: i64) -> Result<Vec<Verdict>> {
    verdicts_where(db, "c.run_id = ?1", run)
}

fn verdicts_where(db: &Connection, condition: &str, value: i64) -> Result<Vec<Verdict>> {
    let sql = format!(
        "SELECT v.call_id, v.rule, v.verdict FROM review_verdict v JOIN review_call c ON c.id = v.call_id JOIN review_run r ON r.id = c.run_id WHERE {condition} ORDER BY v.call_id, v.rule"
    );
    let mut statement = db.prepare(&sql).sql()?;
    let rows = statement
        .query_map(params![value], |row| {
            Ok(Verdict {
                call_id: row.get(0)?,
                rule: row.get(1)?,
                verdict: row.get(2)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// Every landing under review, oldest first.
pub fn all_landings(db: &Connection) -> Result<Vec<LandingRow>> {
    let mut statement = db
        .prepare("SELECT id, key, repo, reviewed_run, landed_run FROM review_landing ORDER BY id")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok(LandingRow {
                id: row.get(0)?,
                key: row.get(1)?,
                repo: row.get(2)?,
                reviewed_run: row.get(3)?,
                landed_run: row.get(4)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// A landing as the store holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandingRow {
    pub id: i64,
    pub key: String,
    pub repo: String,
    /// The run of the last review, whose net change the review has read.
    pub reviewed_run: Option<i64>,
    /// The run that covered the commits pushed, once the change landed.
    pub landed_run: Option<i64>,
}

/// <landing> as the store holds it.
pub fn landing(db: &Connection, landing: i64) -> Result<LandingRow> {
    db.query_row(
        "SELECT id, key, repo, reviewed_run, landed_run FROM review_landing WHERE id = ?1",
        params![landing],
        |row| {
            Ok(LandingRow {
                id: row.get(0)?,
                key: row.get(1)?,
                repo: row.get(2)?,
                reviewed_run: row.get(3)?,
                landed_run: row.get(4)?,
            })
        },
    )
    .optional()
    .sql()?
    .ok_or_else(|| {
        crate::fail::Fail::Message(format!("there is no landing under review {landing}"))
    })
}

/// A run as the store holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRow {
    pub id: i64,
    pub kind: String,
    pub base: String,
    pub head: String,
    /// The run of the last review, when this review run read only the changes
    /// since that run; None for a run that read the whole net change.
    pub since: Option<i64>,
    pub grouping: String,
    /// The commit of the rules checkout the run read.
    pub rules_version: String,
}

impl RunRow {
    /// Whether this run and <other> read the same diff: the same net change,
    /// read whole or as the difference from the same earlier run's.
    pub fn same_diff(&self, other: &RunRow) -> bool {
        self.base == other.base && self.head == other.head && self.since == other.since
    }
}

/// The runs of <landing>.
pub fn runs_of_landing(db: &Connection, landing: i64) -> Result<Vec<RunRow>> {
    let mut statement = db
        .prepare("SELECT id, kind, base, head, since_run, grouping, rules_version FROM review_run WHERE landing_id = ?1 ORDER BY id")
        .sql()?;
    let rows = statement
        .query_map(params![landing], |row| {
            Ok(RunRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                base: row.get(2)?,
                head: row.get(3)?,
                since: row.get(4)?,
                grouping: row.get(5)?,
                rules_version: row.get(6)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// How many landings had a review under the grouping <hash>.
pub fn landings_under(db: &Connection, hash: &str) -> Result<i64> {
    db.query_row(
        "SELECT COUNT(DISTINCT landing_id) FROM review_run WHERE kind = 'review' AND grouping = ?1",
        params![hash],
        |row| row.get(0),
    )
    .sql()
}

/// The landings that had an audit, oldest first.
pub fn audited_landings(db: &Connection) -> Result<Vec<i64>> {
    let mut statement = db
        .prepare(
            "SELECT DISTINCT landing_id FROM review_run WHERE kind = 'audit' ORDER BY landing_id",
        )
        .sql()?;
    let rows = statement.query_map([], |row| row.get(0)).sql()?;
    rows.map(|row| row.sql()).collect()
}

/// Moves a store's review tables from before reviews ran before landing to
/// today's, with the statements taken from record.sql so the result equals a
/// fresh database's: `review_landing` is made again with `landed_at` and
/// `landed_run` in place of `closed_at` (a landing whose review passed landed
/// with its newest review run, since that review ran after the push), and
/// the move adds `review_commit` and `review_fix` empty, as they were then
/// (`BEFORE_CLOSED`). migrate.rs runs it with the foreign keys off, and the rename runs under `legacy_alter_table`, so
/// `review_run`'s reference keeps naming `review_landing`.
pub fn move_to_landing_review(db: &Connection) -> Result<()> {
    let schema = crate::db::REVIEW_PART;
    let slice = |from: &str, end: &str| crate::db::statement(schema, from, end);
    let landing = slice("CREATE TABLE review_landing", "STRICT;");
    let index = slice("CREATE INDEX review_landing_key", ";");
    let commit = crate::db::statement(
        BEFORE_CLOSED,
        "CREATE TABLE review_commit",
        "WITHOUT ROWID;",
    );
    let fix = crate::db::statement(BEFORE_CLOSED, "CREATE TABLE review_fix", "STRICT;");
    crate::db::remake_table(
        db,
        "review_landing",
        landing,
        |db| {
            db.execute_batch(
                "INSERT INTO review_landing (id, key, repo, opened_at, landed_at, landed_run)
    SELECT o.id, o.key, o.repo, o.opened_at, o.closed_at,
        CASE WHEN o.closed_at IS NULL THEN NULL
            ELSE (SELECT MAX(r.id) FROM review_run r WHERE r.landing_id = o.id AND r.kind = 'review') END
    FROM review_landing_old o;",
            )
            .sql()
        },
        &[index, commit, fix],
    )
}

/// Moves a store's `review_call` from before calls kept their prompt and
/// cost to today's, with the statements taken from record.sql so the result
/// equals a fresh database's. Each call's prompt is read from the system
/// prompt of its first exchange, its passes from its exchanges' passes, and
/// whether it read the commit messages from an exchange over them; its cost
/// stays null, since nobody recorded it. A call with no exchange holds no
/// prompt to read, so the move fails and the store stays as it was.
/// migrate.rs runs it with the foreign keys off, and the rename runs under
/// `legacy_alter_table`, so the tables that point at `review_call` keep naming
/// it.
pub fn move_to_scored_calls(db: &Connection) -> Result<()> {
    let schema = crate::db::REVIEW_PART;
    let slice = |from: &str, end: &str| crate::db::statement(schema, from, end);
    let call = slice("CREATE TABLE review_call", "STRICT;");
    let index = slice("CREATE INDEX review_call_run", ";");
    crate::db::remake_table(db, "review_call", call, copy_scored_calls, &[index])
}

/// Copies every call of `review_call_old` into `review_call` with the prompt
/// read from the system prompt of its first exchange, for
/// `move_to_scored_calls`.
fn copy_scored_calls(db: &Connection) -> Result<()> {
    let ids: Vec<i64> = {
        let mut statement = db
            .prepare("SELECT id FROM review_call_old ORDER BY id")
            .sql()?;
        let rows = statement.query_map([], |row| row.get(0)).sql()?;
        rows.map(|row| row.sql()).collect::<Result<_>>()?
    };
    for id in ids {
        let (system, passes, commit_text): (Option<String>, Option<i64>, Option<bool>) = db
            .query_row(
                "SELECT (SELECT t.text FROM review_exchange e JOIN review_text t ON t.hash = e.system_hash WHERE e.call_id = ?1 ORDER BY e.pass, e.id LIMIT 1), MAX(pass) + 1, MAX(unit = ?2) FROM review_exchange WHERE call_id = ?1",
                params![id, crate::review::files::COMMIT_MESSAGE],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .sql()?;
        let (Some(system), Some(passes), Some(commit_text)) = (system, passes, commit_text) else {
            fail!("review call {id} has no exchange to read its prompt from");
        };
        let Some(prompt) =
            crate::review::prompt::setup_of_system(&system, passes as usize, commit_text)
        else {
            fail!("the system prompt of review call {id} holds no group part");
        };
        let prompt = keep_text(db, &prompt)?;
        db.execute(
            "INSERT INTO review_call (id, run_id, group_name, model, prompt, rules, tokens_in, tokens_out, tokens_cached, tokens_written, cost_microdollars, sites, failure)
                SELECT id, run_id, group_name, model, ?2, rules, tokens_in, tokens_out, tokens_cached, tokens_written, NULL, sites, failure
                FROM review_call_old WHERE id = ?1",
            params![id, prompt],
        )
        .sql()?;
    }
    Ok(())
}

/// Moves a store's `review_run` from before a later review run read only how
/// the branch's net change differs from the net change an earlier run read to
/// today's, with the statements taken from record.sql so the result equals a
/// fresh database's: every run keeps its rows, and the move sets `since_run`
/// to null for each, so builds counts each run as one that read the whole net
/// change from its base to its head. migrate.rs runs it with the
/// foreign keys off, and the rename runs under `legacy_alter_table`, so the
/// tables that point at `review_run` keep naming it.
pub fn move_to_net_change(db: &Connection) -> Result<()> {
    let schema = crate::db::REVIEW_PART;
    let slice = |from: &str, end: &str| crate::db::statement(schema, from, end);
    let run = slice("CREATE TABLE review_run", "STRICT;");
    let index = slice("CREATE INDEX review_run_landing", ";");
    crate::db::remake_table(
        db,
        "review_run",
        run,
        |db| {
            db.execute_batch(
                "INSERT INTO review_run (id, landing_id, kind, label, at, base, head, since_run, grouping, rules_version)
    SELECT id, landing_id, kind, label, at, base, head, NULL, grouping, rules_version FROM review_run_old;",
            )
            .sql()
        },
        &[index],
    )
}

/// Moves a store's `review_landing` from before each landing named the run of
/// its last review to today's, with the statements taken from record.sql so
/// the result equals a fresh database's. The move picks as each landing's last
/// review the newest of two kinds of review run: the landing's first run, and
/// a run that read the changes since an earlier review. The move skips a run
/// that has the same base, head and earlier review as an older run of the
/// landing: such a run asked the groups that gave the older run no answer,
/// over the same diff. The move also skips every other later run with no earlier
/// review. Before builds read the changes since the last review, such a run
/// read only a stretch of the branch's commits. migrate.rs runs the move with
/// the foreign keys off, and the rename runs under `legacy_alter_table`, so
/// `review_run`'s reference keeps naming `review_landing`.
pub fn move_to_reviewed_run(db: &Connection) -> Result<()> {
    let schema = crate::db::REVIEW_PART;
    let slice = |from: &str, end: &str| crate::db::statement(schema, from, end);
    let landing = slice("CREATE TABLE review_landing", "STRICT;");
    let index = slice("CREATE INDEX review_landing_key", ";");
    crate::db::remake_table(
        db,
        "review_landing",
        landing,
        |db| {
            db.execute_batch(
                "INSERT INTO review_landing (id, key, repo, opened_at, reviewed_run, landed_at, landed_run)
    SELECT o.id, o.key, o.repo, o.opened_at,
        (SELECT MAX(r.id) FROM review_run r
            WHERE r.landing_id = o.id AND r.kind = 'review'
            AND (r.since_run IS NOT NULL OR NOT EXISTS (
                SELECT 1 FROM review_run e
                WHERE e.landing_id = o.id AND e.kind = 'review' AND e.id < r.id))
            AND NOT EXISTS (
                SELECT 1 FROM review_run e
                WHERE e.landing_id = o.id AND e.kind = 'review' AND e.id < r.id
                AND e.base = r.base AND e.head = r.head AND e.since_run IS r.since_run)),
        o.landed_at, o.landed_run
    FROM review_landing_old o;",
            )
            .sql()
        },
        &[index],
    )
}

/// The tables of read commits, fixes, pushed commits and findings whose code
/// left their branch, as record.sql wrote them before a review closed the
/// confirmed findings its changes fix. The moves of older stores make them,
/// and `move_to_closed` drops them.
pub const BEFORE_CLOSED: &str = "CREATE TABLE review_commit (
    run_id INTEGER NOT NULL REFERENCES review_run (id),
    position INTEGER NOT NULL CONSTRAINT review_commit_position_not_negative CHECK (position >= 0),
    sha TEXT NOT NULL,
    patch_id TEXT NOT NULL,
    PRIMARY KEY (run_id, position)
) STRICT, WITHOUT ROWID;
CREATE TABLE review_fix (
    finding_id INTEGER PRIMARY KEY REFERENCES review_finding (id),
    sha TEXT NOT NULL,
    patch_id TEXT NOT NULL,
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_fix_by_says_something CHECK (trim(by) <> '')
) STRICT;
CREATE TABLE review_pushed (
    landing_id INTEGER NOT NULL REFERENCES review_landing (id),
    patch_id TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (landing_id, patch_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE review_code_left (
    finding_id INTEGER PRIMARY KEY REFERENCES review_finding (id),
    base TEXT NOT NULL,
    head TEXT NOT NULL,
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_code_left_by_says_something CHECK (trim(by) <> '')
) STRICT;";

/// Moves a store's review tables from before a review closed the confirmed
/// findings its changes fix to today's: the move makes `review_closed`, and
/// the last review of each landing closes each confirmed finding that names a
/// fix and each one whose code builds recorded as no longer in its branch's
/// change. Then the move drops the tables of fixes, of the commits each run read, of
/// the commits each push landed and of findings whose code left their branch.
/// Run it after `move_to_reviewed_run`, which names each landing's last
/// review.
pub fn move_to_closed(db: &Connection) -> Result<()> {
    let closed = crate::db::statement(
        crate::db::REVIEW_PART,
        "CREATE TABLE review_closed",
        "STRICT;",
    );
    db.execute_batch(&format!(
        "{closed}
INSERT INTO review_closed (finding_id, run_id)
    SELECT f.id, l.reviewed_run FROM review_finding f
        JOIN review_call c ON c.id = f.call_id
        JOIN review_run r ON r.id = c.run_id
        JOIN review_landing l ON l.id = r.landing_id
    WHERE r.kind = 'review' AND l.reviewed_run IS NOT NULL AND f.settled = 'confirmed'
        AND (f.id IN (SELECT finding_id FROM review_fix)
            OR f.id IN (SELECT finding_id FROM review_code_left));
DROP TABLE review_fix;
DROP TABLE review_code_left;
DROP TABLE review_commit;
DROP TABLE review_pushed;"
    ))
    .sql()
}

/// The tables record.rs owns.
#[cfg(test)]
const TABLES: [&str; 9] = [
    "review_landing",
    "review_run",
    "review_call",
    "review_text",
    "review_exchange",
    "review_verdict",
    "review_finding",
    "review_settle_again",
    "review_closed",
];

/// Writes a row of one of record.rs's tables as given, the constraints still
/// holding: for tests that need a row of a store they build by hand.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;

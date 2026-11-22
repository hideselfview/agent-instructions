//! What reviews wrote down, in the tables of `record.sql`: this file is the
//! only code that writes them. A change under review keeps one landing, found
//! by its repo and key, until it lands, and names the run of its last review; a run
//! holds the net change it read, the run of the last review when it read only
//! the changes since that run, and one call per group; a call holds a verdict
//! for every rule and the findings; the coordinator settles a finding as confirmed or dropped, and
//! may settle an open confirmed finding again, as dropped. A later review run
//! closes a confirmed finding when the changes it read fix it, and the person
//! may allow an open confirmed finding to land.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::review::texts::{self, Texts};

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
    /// When the process began, seconds since 1970, and how long it ran.
    pub started_at: i64,
    pub ms: i64,
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
    pub cost_microdollars: Option<i64>,
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
    /// A landing's review, with how many lines its diff adds and removes.
    Review {
        changed_lines: i64,
    },
    Replay,
    Audit,
    /// A copy review of a branch (`builds review copy`).
    Copy,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Review { .. } => "review",
            Kind::Replay => "replay",
            Kind::Audit => "audit",
            Kind::Copy => "copy",
        }
    }
}

/// A finding as the store holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub id: i64,
    pub run_id: i64,
    /// `review`, `replay`, `audit` or `copy`.
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

/// The person's words that allowed a confirmed finding to land, and who
/// recorded them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowed {
    pub why: String,
    pub by: String,
}

/// The settles of a finding: one; a first settle and the second that
/// changed it to dropped; or a settle as confirmed and the person's words
/// that allowed the finding to land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settled {
    Once(Settle),
    Again { first: Settle, second: Settle },
    Allowed { confirmed: Settle, allowed: Allowed },
}

impl Settled {
    /// The settle the finding counts as: the second, when there is one. A
    /// finding the person allowed counts as confirmed.
    pub fn last(&self) -> &Settle {
        match self {
            Settled::Once(settle) => settle,
            Settled::Again { second, .. } => second,
            Settled::Allowed { confirmed, .. } => confirmed,
        }
    }
}

/// How the coordinator settled a finding, who did, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settle {
    /// `confirmed` or `dropped`.
    pub how: String,
    pub why: String,
    pub by: String,
    pub at: i64,
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

    /// Whether it is a finding of a review that builds' own checks accepted
    /// and that nobody has settled.
    pub fn is_unsettled(&self) -> bool {
        self.kind == "review" && self.counts() && self.settled.is_none()
    }

    /// Whether <raised> is this finding again: the same rule, file and
    /// quoted code line (`cases::same_code`).
    pub fn matches(&self, raised: &NewFinding) -> bool {
        self.rule == raised.rule
            && self.file == raised.file
            && super::cases::same_code(&self.code, &raised.code)
    }

    /// Whether it is an open confirmed finding of a review: settled once, as
    /// confirmed, and no review run closed it.
    pub fn is_open(&self) -> bool {
        self.why_not_open().is_none()
    }

    /// Why it is not an open confirmed finding of a review, or None when it
    /// is one.
    fn why_not_open(&self) -> Option<String> {
        let id = self.id;
        if let Some(reason) = &self.check_failed {
            return Some(format!("builds' own check dropped finding {id} ({reason})"));
        }
        if self.kind != "review" {
            return Some(format!(
                "finding {id} comes from a run of kind {}, and only a review's findings are open",
                self.kind
            ));
        }
        match &self.settled {
            None => Some(format!(
                "the coordinator has not settled finding {id}; settle it first: builds review settle {id} confirmed|dropped --why TEXT"
            )),
            Some(Settled::Again { .. }) => Some(format!(
                "the coordinator settled finding {id} again already, as dropped"
            )),
            Some(Settled::Allowed { .. }) => Some(format!(
                "the person allowed finding {id} to land already, so it stays confirmed"
            )),
            Some(Settled::Once(settle)) if settle.how != "confirmed" => Some(format!(
                "the coordinator settled finding {id} as {} already",
                settle.how
            )),
            Some(Settled::Once(_)) => self
                .closed_by
                .map(|run| format!("review run {run} closed finding {id}, so it stays confirmed")),
        }
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
    /// The hash its prompt is kept under (texts.rs).
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

/// A change under review: the repo its commits are in, and its key, by
/// default its branch (`branch:<name>`). Branches of the same name in two
/// repos are two changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change<'a> {
    pub repo: &'a str,
    pub key: &'a str,
}

/// The landing of <change> that has not landed, if any.
pub fn open_landing(db: &Connection, change: Change) -> Result<Option<i64>> {
    db.query_row(
        "SELECT id FROM review_landing WHERE repo = ?1 AND key = ?2 AND landed_at IS NULL",
        params![change.repo, change.key],
        |row| row.get(0),
    )
    .optional()
    .sql()
}

/// Opens the landing of <change> at <now>. The database refuses a second
/// open landing of one change.
pub fn add_landing(db: &Connection, change: Change, now: i64) -> Result<i64> {
    db.execute(
        "INSERT INTO review_landing (key, repo, opened_at) VALUES (?1, ?2, ?3)",
        params![change.key, change.repo, now],
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
    /// For a review run that reads only the changes since an earlier run's
    /// net change, that earlier run.
    pub since: Option<i64>,
    /// The commit of the rules checkout the run read.
    pub rules_version: &'a str,
}

/// Moves a store's review texts to <texts>, for a store from before builds
/// kept them there: keeps each run's texts, makes `review_call` and
/// `review_exchange` again as record.sql writes them, with each answer's hash
/// in place of the answer, and drops `review_text`. migrate.rs runs it with the
/// foreign keys off. A text the store has no row for fails the move.
pub fn move_texts_to_volume(db: &Connection, texts: &Texts) -> Result<()> {
    let runs: Vec<i64> = db
        .prepare("SELECT id FROM review_run ORDER BY id")
        .sql()?
        .query_map([], |row| row.get(0))
        .sql()?
        .collect::<rusqlite::Result<_>>()
        .sql()?;
    for run in runs {
        let mut held = db
            .prepare(
                "SELECT t.text FROM review_call c JOIN review_text t ON t.hash = c.prompt WHERE c.run_id = ?1
                 UNION ALL
                 SELECT t.text FROM review_exchange e JOIN review_call c ON c.id = e.call_id JOIN review_text t ON t.hash IN (e.system_hash, e.message_hash) WHERE c.run_id = ?1
                 UNION ALL
                 SELECT e.output FROM review_exchange e JOIN review_call c ON c.id = e.call_id WHERE c.run_id = ?1",
            )
            .sql()?;
        let held: Vec<String> = held
            .query_map(params![run], |row| row.get(0))
            .sql()?
            .collect::<rusqlite::Result<_>>()
            .sql()?;
        texts.keep_run(run, held.iter().map(String::as_str))?;
    }
    crate::db::remake_table(
        db,
        "review_call",
        crate::db::statement(
            crate::db::REVIEW_PART,
            "CREATE TABLE review_call",
            ") STRICT;",
        ),
        |db| {
            db.execute_batch("INSERT INTO review_call SELECT * FROM review_call_old")
                .sql()
        },
        &[crate::db::statement(
            crate::db::REVIEW_PART,
            "CREATE INDEX review_call_run",
            ";",
        )],
    )?;
    crate::db::remake_table(
        db,
        "review_exchange",
        crate::db::statement(
            crate::db::REVIEW_PART,
            "CREATE TABLE review_exchange",
            ") STRICT;",
        ),
        |db| {
            let mut old = db
                .prepare(
                    "SELECT id, call_id, model, unit, pass, system_hash, message_hash, output, tokens_in, tokens_out, tokens_cached, tokens_written FROM review_exchange_old",
                )
                .sql()?;
            let mut rows = old.query([]).sql()?;
            while let Some(row) = rows.next().sql()? {
                let output: String = row.get(7).sql()?;
                db.execute(
                    "INSERT INTO review_exchange (id, call_id, model, unit, pass, system_hash, message_hash, output_hash, tokens_in, tokens_out, tokens_cached, tokens_written) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        row.get::<_, i64>(0).sql()?,
                        row.get::<_, i64>(1).sql()?,
                        row.get::<_, String>(2).sql()?,
                        row.get::<_, String>(3).sql()?,
                        row.get::<_, i64>(4).sql()?,
                        row.get::<_, String>(5).sql()?,
                        row.get::<_, String>(6).sql()?,
                        texts::hash(&output),
                        row.get::<_, i64>(8).sql()?,
                        row.get::<_, i64>(9).sql()?,
                        row.get::<_, i64>(10).sql()?,
                        row.get::<_, i64>(11).sql()?,
                    ],
                )
                .sql()?;
            }
            Ok(())
        },
        &[crate::db::statement(
            crate::db::REVIEW_PART,
            "CREATE INDEX review_exchange_call",
            ";",
        )],
    )?;
    db.execute_batch("DROP TABLE review_text;").sql()
}

/// Writes a run with its calls, verdicts and findings, and closes each
/// finding a call closes, and keeps the run's texts in <texts>. Returns the
/// run's id and, per call in order, the ids its findings got. The texts are
/// kept after the rows, so a failure to keep them fails the write, which the
/// caller's transaction then rolls back.
pub fn add_run(
    db: &Connection,
    texts: &Texts,
    run: &NewRun,
    calls: &[NewCall],
) -> Result<(i64, Vec<Vec<i64>>)> {
    let changed_lines = match run.kind {
        Kind::Review { changed_lines } => Some(changed_lines),
        Kind::Replay | Kind::Audit | Kind::Copy => None,
    };
    db.execute(
        "INSERT INTO review_run (landing_id, kind, label, at, base, head, since_run, rules_version, changed_lines) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![run.landing, run.kind.word(), run.label, run.at, run.base, run.head, run.since, run.rules_version, changed_lines],
    )
    .sql()?;
    let run_id = db.last_insert_rowid();
    let ids = add_calls(db, run_id, run.landing, calls)?;
    texts.keep_run(
        run_id,
        calls.iter().flat_map(|call| {
            std::iter::once(call.prompt.as_str()).chain(
                call.exchanges
                    .iter()
                    .flat_map(|e| [e.system.as_str(), e.message.as_str(), e.output.as_str()]),
            )
        }),
    )?;
    Ok((run_id, ids))
}

/// Writes <calls> of run <run_id>, a run of <landing>, with their verdicts,
/// exchanges and findings, and closes each finding a call closes. Returns,
/// per call in order, the ids its findings got.
pub fn add_calls(
    db: &Connection,
    run_id: i64,
    landing: Option<i64>,
    calls: &[NewCall],
) -> Result<Vec<Vec<i64>>> {
    let mut ids = Vec::new();
    for call in calls {
        let rules = serde_json::to_string(&call.rules).expect("strings serialize");
        let prompt = texts::hash(&call.prompt);
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
            let system_hash = texts::hash(&exchange.system);
            let message_hash = texts::hash(&exchange.message);
            let output_hash = texts::hash(&exchange.output);
            db.execute(
                "INSERT INTO review_exchange (call_id, model, unit, pass, system_hash, message_hash, output_hash, tokens_in, tokens_out, tokens_cached, tokens_written) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![call_id, exchange.model, exchange.unit, exchange.pass, system_hash, message_hash, output_hash, exchange.tokens_in, exchange.tokens_out, exchange.tokens_cached, exchange.tokens_written],
            )
            .sql()?;
            crate::review::exchange_time::record(
                db,
                db.last_insert_rowid(),
                exchange.started_at,
                exchange.ms,
            )?;
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
            close(db, *finding, run_id, landing)?;
        }
    }
    Ok(ids)
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

/// Finding <id>, when it is an open confirmed finding of a review; a
/// failure that says why it is not, otherwise.
fn open_finding(db: &Connection, id: i64) -> Result<Finding> {
    let finding = finding_of(db, id)?;
    match finding.why_not_open() {
        None => Ok(finding),
        Some(why) => fail!(
            "{why}. The coordinator settles again, and the person allows, only an open confirmed finding."
        ),
    }
}

/// Settles finding <id> again, as dropped: only an open confirmed finding
/// changes. The finding keeps its first settle beside the second,
/// and counts as the second from then on.
pub fn settle_again(db: &Connection, id: i64, why: &str, now: i64, by: &str) -> Result<()> {
    if why.trim().is_empty() {
        fail!("--why says nothing");
    }
    open_finding(db, id)?;
    db.execute(
        "INSERT INTO review_settle_again (finding_id, why, at, by) VALUES (?1, ?2, ?3, ?4)",
        params![id, why, now, by],
    )
    .sql()?;
    Ok(())
}

/// Records that the person allowed open confirmed finding <id> to land, with
/// <why> holding their words. The finding stays confirmed and is no longer
/// open.
pub fn allow(db: &Connection, id: i64, why: &str, now: i64, by: &str) -> Result<()> {
    open_finding(db, id)?;
    db.execute(
        "INSERT INTO review_allowed (finding_id, why, at, by) VALUES (?1, ?2, ?3, ?4)",
        params![id, why, now, by],
    )
    .sql()?;
    Ok(())
}

const FINDING_COLUMNS: &str = "f.id, c.run_id, r.kind, r.landing_id, c.group_name, f.rule, f.clause, f.file, f.line, f.code, f.problem, f.blocking, f.check_failed, f.settled, f.settled_why, f.settled_by, a.why, a.by, z.run_id, w.why, w.by, f.settled_at, a.at";

fn finding_from(row: &rusqlite::Row) -> rusqlite::Result<Finding> {
    let settled: Option<String> = row.get(13)?;
    let why: Option<String> = row.get(14)?;
    let by: Option<String> = row.get(15)?;
    let again_why: Option<String> = row.get(16)?;
    let again_by: Option<String> = row.get(17)?;
    let allowed_why: Option<String> = row.get(19)?;
    let allowed_by: Option<String> = row.get(20)?;
    let at: Option<i64> = row.get(21)?;
    let again_at: Option<i64> = row.get(22)?;
    let first = match (settled, why, by, at) {
        (Some(how), Some(why), Some(by), Some(at)) => Some(Settle { how, why, by, at }),
        _ => None,
    };
    let again = match (again_why, again_by, again_at) {
        (Some(why), Some(by), Some(at)) => Some(Settle {
            how: "dropped".to_string(),
            why,
            by,
            at,
        }),
        _ => None,
    };
    let allowed = allowed_why
        .zip(allowed_by)
        .map(|(why, by)| Allowed { why, by });
    let settled = match (first, again, allowed) {
        (Some(_), Some(_), Some(_)) => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                16,
                rusqlite::types::Type::Text,
                "the coordinator settled the finding again and the person allowed it to land, but a finding holds only one of these".into(),
            ));
        }
        (Some(first), Some(second), None) => Some(Settled::Again { first, second }),
        (Some(confirmed), None, Some(allowed)) => Some(Settled::Allowed { confirmed, allowed }),
        (Some(first), None, None) => Some(Settled::Once(first)),
        (None, _, _) => None,
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

const FINDING_FROM: &str = "FROM review_finding f JOIN review_call c ON c.id = f.call_id JOIN review_run r ON r.id = c.run_id LEFT JOIN review_settle_again a ON a.finding_id = f.id LEFT JOIN review_closed z ON z.finding_id = f.id LEFT JOIN review_allowed w ON w.finding_id = f.id";

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

/// The messages builds sent to each process of the call of <group> in <run>,
/// in the order it sent them, read from <texts>. Fails when the cap deleted
/// the run's texts.
pub fn messages_of_call(
    db: &Connection,
    texts: &Texts,
    run: i64,
    group: &str,
) -> Result<Vec<String>> {
    let mut statement = db
        .prepare(
            "SELECT e.message_hash FROM review_exchange e JOIN review_call c ON c.id = e.call_id WHERE c.run_id = ?1 AND c.group_name = ?2 ORDER BY e.id",
        )
        .sql()?;
    let hashes = statement
        .query_map(params![run, group], |row| row.get::<_, String>(0))
        .sql()?
        .collect::<rusqlite::Result<Vec<String>>>()
        .sql()?;
    hashes.iter().map(|hash| texts.read(run, hash)).collect()
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
    /// When the run started.
    pub at: i64,
    pub base: String,
    pub head: String,
    /// The earlier run, when this review run read only the changes since that
    /// run's net change; None for a run that read the whole net change.
    pub since: Option<i64>,
    /// The commit of the rules checkout the run read.
    pub rules_version: String,
}

impl RunRow {
    /// Whether this run and <other> read the same diff: the same net change,
    /// read whole or as the difference from the same earlier run's.
    pub fn same_diff(&self, other: &RunRow) -> bool {
        self.reads_up_to(other) && self.since == other.since
    }

    /// Whether this run and <other> read up to the same net change, whole or
    /// as the difference from any earlier run's.
    pub fn reads_up_to(&self, other: &RunRow) -> bool {
        self.base == other.base && self.head == other.head
    }
}

/// The runs of <landing>.
pub fn runs_of_landing(db: &Connection, landing: i64) -> Result<Vec<RunRow>> {
    let mut statement = db
        .prepare("SELECT id, kind, at, base, head, since_run, rules_version FROM review_run WHERE landing_id = ?1 ORDER BY id")
        .sql()?;
    let rows = statement
        .query_map(params![landing], |row| {
            Ok(RunRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                at: row.get(2)?,
                base: row.get(3)?,
                head: row.get(4)?,
                since: row.get(5)?,
                rules_version: row.get(6)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
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

/// A branch's first review run: when it ran, how many lines its diff adds and
/// removes (None for a run from before builds counted them), the groups whose
/// calls answered, and the findings whose last settle is confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstReview {
    pub at: i64,
    pub changed_lines: Option<i64>,
    pub groups: Vec<String>,
    pub confirmed: Vec<Finding>,
}

/// The id of each landing's first review run.
const FIRST_RUNS: &str = "SELECT MIN(id) FROM review_run WHERE kind = 'review' GROUP BY landing_id";

/// The first review run of each branch, oldest first. A finding counts as its
/// last settle, so the list leaves out a finding settled again as dropped.
/// <db> must not be inside a transaction: the read takes its own.
pub fn first_reviews(db: &Connection) -> Result<Vec<FirstReview>> {
    // The function reads in one transaction, so the runs, calls and findings
    // come from one state of the store even when a review records a run
    // meanwhile.
    let read = db.unchecked_transaction().sql()?;
    let mut reviews: Vec<(i64, FirstReview)> = {
        let mut statement = read
            .prepare(&format!(
                "SELECT id, at, changed_lines FROM review_run WHERE id IN ({FIRST_RUNS}) ORDER BY id"
            ))
            .sql()?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    FirstReview {
                        at: row.get(1)?,
                        changed_lines: row.get(2)?,
                        groups: Vec::new(),
                        confirmed: Vec::new(),
                    },
                ))
            })
            .sql()?;
        rows.map(|row| row.sql()).collect::<Result<_>>()?
    };
    let review_of = |reviews: &[(i64, FirstReview)], run: i64| -> usize {
        reviews
            .iter()
            .position(|(id, _)| *id == run)
            .expect("each call and finding read here belongs to a first review run")
    };
    let groups: Vec<(i64, String)> = {
        let mut statement = read
            .prepare(&format!(
                "SELECT run_id, group_name FROM review_call WHERE failure IS NULL AND run_id IN ({FIRST_RUNS}) ORDER BY id"
            ))
            .sql()?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .sql()?;
        rows.map(|row| row.sql()).collect::<Result<_>>()?
    };
    for (run, group) in groups {
        let at = review_of(&reviews, run);
        reviews[at].1.groups.push(group);
    }
    let findings: Vec<Finding> = {
        let sql = format!(
            "SELECT {FINDING_COLUMNS} {FINDING_FROM} WHERE c.run_id IN ({FIRST_RUNS}) ORDER BY f.id"
        );
        let mut statement = read.prepare(&sql).sql()?;
        let rows = statement.query_map([], finding_from).sql()?;
        rows.map(|row| row.sql()).collect::<Result<_>>()?
    };
    for finding in findings {
        if finding.settled_as("confirmed") {
            let at = review_of(&reviews, finding.run_id);
            reviews[at].1.confirmed.push(finding);
        }
    }
    Ok(reviews.into_iter().map(|(_, review)| review).collect())
}

#[cfg(test)]
#[path = "record_tests.rs"]
pub mod tests;

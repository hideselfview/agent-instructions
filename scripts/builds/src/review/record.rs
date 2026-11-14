//! What reviews wrote down, in the tables of `record.sql`: this file is the
//! only code that writes them. A landing under review keeps one key across
//! its reruns; a run holds one call per group, a call holds a verdict for every
//! rule and the findings; the coordinator settles a finding as confirmed or
//! dropped.

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

/// One reviewer call, as it goes into the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCall {
    pub group: String,
    pub model: String,
    pub rules: Vec<String>,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub failure: Option<String>,
    /// (rule, `finding` or `none`).
    pub verdicts: Vec<(String, String)>,
    pub findings: Vec<NewFinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Review,
    Replay,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Review => "review",
            Kind::Replay => "replay",
        }
    }
}

/// A finding as the store holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub id: i64,
    pub run_id: i64,
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
    /// (`confirmed` or `dropped`, why).
    pub settled: Option<(String, String)>,
}

impl Finding {
    /// Whether builds' own checks accepted it: a finding the coordinator sees.
    pub fn counts(&self) -> bool {
        self.check_failed.is_none()
    }

    /// Whether it refuses a landing: blocking, accepted and not dropped.
    pub fn refuses(&self) -> bool {
        self.counts()
            && self.blocking
            && !matches!(&self.settled, Some((how, _)) if how == "dropped")
    }
}

/// A call as the store holds it, without its findings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub id: i64,
    pub run_id: i64,
    pub group: String,
    pub model: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub failure: Option<String>,
}

/// The landing of <key> that no review has passed yet, if any.
pub fn open_landing(db: &Connection, key: &str) -> Result<Option<i64>> {
    db.query_row(
        "SELECT id FROM review_landing WHERE key = ?1 AND closed_at IS NULL ORDER BY id DESC LIMIT 1",
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

/// Marks the landing's review passed.
pub fn close_landing(db: &Connection, landing: i64, now: i64) -> Result<()> {
    db.execute(
        "UPDATE review_landing SET closed_at = ?2 WHERE id = ?1 AND closed_at IS NULL",
        params![landing, now],
    )
    .sql()?;
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
}

/// Writes a run with its calls, verdicts and findings. Returns the run's id
/// and, per call in order, the ids its findings got.
pub fn add_run(db: &Connection, run: &NewRun, calls: &[NewCall]) -> Result<(i64, Vec<Vec<i64>>)> {
    db.execute(
        "INSERT INTO review_run (landing_id, kind, label, at, base, head) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![run.landing, run.kind.word(), run.label, run.at, run.base, run.head],
    )
    .sql()?;
    let run = db.last_insert_rowid();
    let mut ids = Vec::new();
    for call in calls {
        let rules = serde_json::to_string(&call.rules).expect("strings serialize");
        db.execute(
            "INSERT INTO review_call (run_id, group_name, model, rules, tokens_in, tokens_out, failure) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![run, call.group, call.model, rules, call.tokens_in, call.tokens_out, call.failure],
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
    }
    Ok((run, ids))
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
        fail!(
            "finding {id} was dropped by builds' own check ({reason}); there is nothing to settle"
        );
    }
    if let Some(settled) = settled {
        fail!("finding {id} is settled already, as {settled}");
    }
    db.execute(
        "UPDATE review_finding SET settled = ?2, settled_why = ?3, settled_at = ?4, settled_by = ?5 WHERE id = ?1",
        params![id, how, why, now, by],
    )
    .sql()?;
    Ok(())
}

const FINDING_COLUMNS: &str = "f.id, c.run_id, r.landing_id, c.group_name, f.rule, f.clause, f.file, f.line, f.code, f.problem, f.blocking, f.check_failed, f.settled, f.settled_why";

fn finding_from(row: &rusqlite::Row) -> rusqlite::Result<Finding> {
    let settled: Option<String> = row.get(12)?;
    let why: Option<String> = row.get(13)?;
    Ok(Finding {
        id: row.get(0)?,
        run_id: row.get(1)?,
        landing_id: row.get(2)?,
        group: row.get(3)?,
        rule: row.get(4)?,
        clause: row.get(5)?,
        file: row.get(6)?,
        line: row.get(7)?,
        code: row.get(8)?,
        problem: row.get(9)?,
        blocking: row.get(10)?,
        check_failed: row.get(11)?,
        settled: settled.zip(why),
    })
}

const FINDING_FROM: &str = "FROM review_finding f JOIN review_call c ON c.id = f.call_id JOIN review_run r ON r.id = c.run_id";

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

fn calls_where(db: &Connection, condition: &str, value: i64) -> Result<Vec<Call>> {
    let sql = format!(
        "SELECT c.id, c.run_id, c.group_name, c.model, c.tokens_in, c.tokens_out, c.failure FROM review_call c JOIN review_run r ON r.id = c.run_id WHERE {condition} ORDER BY c.id"
    );
    let mut statement = db.prepare(&sql).sql()?;
    let rows = statement
        .query_map(params![value], |row| {
            Ok(Call {
                id: row.get(0)?,
                run_id: row.get(1)?,
                group: row.get(2)?,
                model: row.get(3)?,
                tokens_in: row.get(4)?,
                tokens_out: row.get(5)?,
                failure: row.get(6)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// (id, key, repo, closed) of <landing>.
pub fn landing(db: &Connection, landing: i64) -> Result<(String, String, bool)> {
    db.query_row(
        "SELECT key, repo, closed_at IS NOT NULL FROM review_landing WHERE id = ?1",
        params![landing],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .sql()?
    .ok_or_else(|| {
        crate::fail::Fail::Message(format!("there is no landing under review {landing}"))
    })
}

/// A run as the store holds it.
pub struct RunRow {
    pub id: i64,
    pub kind: String,
    pub base: String,
    pub head: String,
}

/// The runs of <landing>.
pub fn runs_of_landing(db: &Connection, landing: i64) -> Result<Vec<RunRow>> {
    let mut statement = db
        .prepare("SELECT id, kind, base, head FROM review_run WHERE landing_id = ?1 ORDER BY id")
        .sql()?;
    let rows = statement
        .query_map(params![landing], |row| {
            Ok(RunRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                base: row.get(2)?,
                head: row.get(3)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// The groups of <landing> that need another look: a call that failed or one
/// that raised a finding builds accepted.
pub fn groups_to_rerun(db: &Connection, landing: i64) -> Result<Vec<String>> {
    let mut groups: Vec<String> = Vec::new();
    for call in calls_of_landing(db, landing)? {
        if call.failure.is_some() && !groups.contains(&call.group) {
            groups.push(call.group);
        }
    }
    for finding in findings_of_landing(db, landing)? {
        if finding.counts() && !groups.contains(&finding.group) {
            groups.push(finding.group);
        }
    }
    Ok(groups)
}

/// Marks finding <id> settled as `dropped` because it repeats <earlier>.
pub fn inherit_drop(db: &Connection, id: i64, earlier: &Finding, now: i64) -> Result<()> {
    let why = format!(
        "the same finding as {}, settled dropped: {}",
        earlier.id,
        earlier
            .settled
            .as_ref()
            .map(|(_, why)| why.as_str())
            .unwrap_or("")
    );
    settle(db, id, "dropped", &why, now, "builds")
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;

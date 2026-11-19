//! What builds told the judge: every review finding that waits for the judge
//! reaches it, and none sits unseen while a judge listens.
//!
//! A finding waits for the judge in two ways (`Matter`): nobody has settled
//! it, or someone disputed it as confirmed and the judge has not answered.
//! builds records each matter it told a judge's session of
//! (`review_judge_told`, `judge_told.sql`; this is the only code that writes
//! it). `tell_waiting` sends the judge that listens every waiting matter it
//! has not told that session of, grouped by landing:
//!
//! - when a session starts listening (`builds judge listen`), because the
//!   findings raised before it listened reached no judge;
//! - after each settle, dispute or answer, so that a finding that waits again
//!   because of it reaches the judge too;
//! - from `builds health`, which sends again each matter that waited longer
//!   than `QUIET_SECONDS` and reached no judge, and forgets the judge whose
//!   session ended (`forget_ended`), so that the coordinator settles.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::inbox::Sent;
use crate::lists::Stamp;
use crate::lists::store::Store;
use crate::review::judge::{self, Judge, ToSettle, Waiting};
use crate::review::record;

/// How long a matter waits before `builds health` sends it again.
pub const QUIET_SECONDS: i64 = 5 * 60;

/// What a finding asks of the judge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matter {
    /// Settle the finding.
    Settle,
    /// Answer the dispute of the confirmed finding.
    Answer,
}

impl Matter {
    fn word(self) -> &'static str {
        match self {
            Matter::Settle => "settle",
            Matter::Answer => "answer",
        }
    }
}

/// Records that builds told the judge's session <session> of <matters>, at
/// <stamp>. A matter told again keeps its newest row.
pub fn record(
    store: &Store,
    stamp: &Stamp,
    matters: &[(i64, Matter)],
    session: &str,
) -> Result<()> {
    store.write(stamp, "judge told", |db| {
        for (finding, matter) in matters {
            db.execute(
                "INSERT OR REPLACE INTO review_judge_told (finding_id, matter, session, at) VALUES (?1, ?2, ?3, ?4)",
                params![finding, matter.word(), session, stamp.at],
            )
            .sql()?;
        }
        Ok(())
    })
}

/// Whether builds told <session> of <matter> of finding <finding>.
fn told_to(db: &Connection, finding: i64, matter: Matter, session: &str) -> Result<bool> {
    let told: Option<String> = db
        .query_row(
            "SELECT session FROM review_judge_told WHERE finding_id = ?1 AND matter = ?2",
            params![finding, matter.word()],
            |row| row.get(0),
        )
        .optional()
        .sql()?;
    Ok(told.as_deref() == Some(session))
}

/// Every matter of the project that waits for the judge, oldest landing
/// first.
fn waiting_in(db: &Connection) -> Result<Vec<Waiting>> {
    let mut found = Vec::new();
    for landing in record::all_landings(db)? {
        found.extend(judge::waiting_matters(db, landing.id)?);
    }
    Ok(found)
}

/// The matters that wait for <judge> and that builds has not told its session
/// of, among those that have waited <quiet> seconds or more at <now>.
fn untold(db: &Connection, judge: &Judge, now: i64, quiet: i64) -> Result<Vec<Waiting>> {
    let mut found = Vec::new();
    for waiting in waiting_in(db)? {
        if now - waiting.since >= quiet
            && !told_to(db, waiting.finding.id, waiting.matter, &judge.session)?
        {
            found.push(waiting);
        }
    }
    Ok(found)
}

/// Sends the project's judge, when one listens (Claude Code's running
/// sessions are in <sessions>), each matter that has waited <quiet> seconds
/// or more at <stamp> and that builds has not told the judge's session of:
/// one message for the findings to settle of each landing, and one for each
/// dispute to answer. Records each matter a message reached. Returns a line
/// for each message sent, and one when the judge's inbox is gone, which
/// leaves its matters untold.
pub fn tell_waiting(
    store: &Store,
    stamp: &Stamp,
    sessions: &Path,
    quiet: i64,
) -> Result<Vec<String>> {
    let conn = store.open_existing()?;
    let Some(judge) = judge::listening(&conn, sessions)? else {
        return Ok(Vec::new());
    };
    let waiting = untold(&conn, &judge, stamp.at, quiet)?;
    let mut notices = Vec::new();
    let mut landings: Vec<i64> = Vec::new();
    for waited in &waiting {
        if waited.matter == Matter::Settle {
            let landing = judge::landing_of(&waited.finding)?;
            if !landings.contains(&landing) {
                landings.push(landing);
            }
        }
    }
    for landing in landings {
        let to_settle = ToSettle {
            project: store.project.clone(),
            landing,
            key: record::landing(&conn, landing)?.key,
            worktree: None,
            findings: waiting
                .iter()
                .filter(|w| w.matter == Matter::Settle && w.finding.landing_id == Some(landing))
                .map(|w| w.finding.id)
                .collect(),
        };
        if let Some(notice) = to_settle.notice() {
            let line = format!(
                "builds told the judge {} of findings {} of review landing #{landing}, which waited for a settle",
                judge.by,
                to_settle.ids()
            );
            notices.push((notice, line));
        }
    }
    for waited in waiting.iter().filter(|w| w.matter == Matter::Answer) {
        let notice = judge::dispute_notice(&conn, &store.project, &waited.finding)?;
        let line = format!(
            "builds told the judge {} of the dispute of finding {}, which waited for an answer",
            judge.by, waited.finding.id
        );
        notices.push((notice, line));
    }
    let mut lines = Vec::new();
    for (notice, line) in notices {
        match judge::tell_judge(&judge, &notice)? {
            Sent::Delivered => {
                record(store, stamp, &notice.matters, &judge.session)?;
                lines.push(line);
            }
            Sent::Gone => {
                lines.push(format!(
                    "the inbox of the judge {} of project {} is gone, so builds could not tell it of findings that wait",
                    judge.by, store.project
                ));
                break;
            }
        }
    }
    Ok(lines)
}

/// Forgets the project's judge when its session no longer runs, so that the
/// coordinator settles the findings that wait; returns the line that says
/// so, naming the findings that wait, and None when no judge ended.
pub fn forget_ended(store: &Store, stamp: &Stamp, sessions: &Path) -> Result<Option<String>> {
    let conn = store.open_existing()?;
    let Some(judge) = judge::ended(&conn, sessions)? else {
        return Ok(None);
    };
    let waiting: Vec<String> = waiting_in(&conn)?
        .iter()
        .map(|w| w.finding.id.to_string())
        .collect();
    store.write(stamp, "judge forgotten", judge::forget)?;
    Ok(Some(format!(
        "the session of project {}'s judge {} ended, so builds forgot it and the coordinator settles findings now{}",
        store.project,
        judge.by,
        if waiting.is_empty() {
            String::new()
        } else {
            format!(": findings {} wait", waiting.join(", "))
        }
    )))
}

#[cfg(test)]
#[path = "judge_told_tests.rs"]
mod tests;

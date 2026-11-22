//! What builds told the judge: every review finding that waits for the judge
//! reaches it, and none sits unseen while a judge listens.
//!
//! A finding waits for the judge in two ways (`Matter`): nobody has settled
//! it, or someone disputed it as confirmed and the judge has not answered.
//! The builds process that makes a finding wait writes its row in
//! `review_judge_told` (`judge_told.sql`; this is the only code that writes
//! it) in the same transaction (`begin`), naming itself by its pid and start
//! time, and then tells the judge, or the coordinator, itself. Telling a
//! judge's session of a matter marks the row told (`record`).
//!
//! `tell_waiting` sends the judge that listens each waiting matter that no
//! running process is telling the judge of: a matter whose row is told to
//! another session than the judge's, or has no row, and whose writer no longer
//! runs (the process table says, never how long the matter waited). It runs:
//!
//! - when a session starts listening (`builds judge listen`), because the
//!   findings raised before it listened reached no judge;
//! - after each settle, dispute and answer, so that a finding that waits again
//!   because of it reaches the judge too;
//! - from `builds health`, which also forgets the judge whose session ended
//!   (`forget_ended`), so that the coordinator settles.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::inbox::Sent;
use crate::lists::Stamp;
use crate::lists::store::Store;
use crate::procs::Processes;
use crate::review::judge::{self, Judge, ToSettle, Waiting};
use crate::review::record;

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

/// A builds process, named so that another process with the same pid, started
/// later, is another one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Writer {
    pid: i32,
    started: i64,
}

impl Writer {
    /// This process, as the process table lists it.
    fn this_process() -> Result<Writer> {
        let pid = std::process::id() as i32;
        match Processes::read()?.get(pid) {
            Some(process) => Ok(Writer {
                pid,
                started: process.started,
            }),
            None => fail!("the process table does not list this process ({pid})"),
        }
    }

    /// Whether the process <processes> lists under this pid is the one that
    /// started at this time.
    fn runs(self, processes: &Processes) -> bool {
        processes
            .get(self.pid)
            .is_some_and(|process| process.started == self.started)
    }
}

/// Writes the rows of <matters>, not yet told, for this process, in the
/// transaction that makes them wait. A row of the same matter that exists
/// is replaced: the matter waits again.
pub fn begin(db: &Connection, matters: &[(i64, Matter)]) -> Result<()> {
    if matters.is_empty() {
        return Ok(());
    }
    let writer = Writer::this_process()?;
    for (finding, matter) in matters {
        db.execute(
            "INSERT OR REPLACE INTO review_judge_told (finding_id, matter, writer_pid, writer_started) VALUES (?1, ?2, ?3, ?4)",
            params![finding, matter.word(), writer.pid, writer.started],
        )
        .sql()?;
    }
    Ok(())
}

/// Marks <matters> told to the judge's session <session>, at <stamp>. A
/// matter with no row (it waited before builds wrote rows) gets one that
/// names no running process.
pub fn record(
    store: &Store,
    stamp: &Stamp,
    matters: &[(i64, Matter)],
    session: &str,
) -> Result<()> {
    store.write(stamp, "judge told", |db| {
        for (finding, matter) in matters {
            db.execute(
                "INSERT INTO review_judge_told (finding_id, matter, writer_pid, writer_started, told_session, told_at) VALUES (?1, ?2, 0, 0, ?3, ?4) ON CONFLICT (finding_id, matter) DO UPDATE SET told_session = ?3, told_at = ?4",
                params![finding, matter.word(), session, stamp.at],
            )
            .sql()?;
        }
        Ok(())
    })
}

/// The session builds told of <matter> of finding <finding>, and the
/// process that wrote its row; a process that runs never, when it has no
/// row.
fn tell_of(db: &Connection, finding: i64, matter: Matter) -> Result<(Option<String>, Writer)> {
    let row: Option<(Option<String>, i32, i64)> = db
        .query_row(
            "SELECT told_session, writer_pid, writer_started FROM review_judge_told WHERE finding_id = ?1 AND matter = ?2",
            params![finding, matter.word()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .sql()?;
    Ok(match row {
        Some((told, pid, started)) => (told, Writer { pid, started }),
        None => (None, Writer { pid: 0, started: 0 }),
    })
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

/// The matters that wait for <judge> and that no builds process is still
/// telling its session of.
fn untold(db: &Connection, judge: &Judge) -> Result<Vec<Waiting>> {
    let mut candidates = Vec::new();
    for waiting in waiting_in(db)? {
        if judge::owner_matches(db, &waiting.finding, |owner| judge.is(owner))? {
            continue;
        }
        let (told, writer) = tell_of(db, waiting.finding.id, waiting.matter)?;
        if told.as_deref() != Some(judge.session.as_str()) {
            candidates.push((waiting, writer));
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let processes = Processes::read()?;
    Ok(candidates
        .into_iter()
        .filter(|(_, writer)| !writer.runs(&processes))
        .map(|(waiting, _)| waiting)
        .collect())
}

/// Sends the project's judge, when one listens (Claude Code's running
/// sessions are in <sessions>), each matter that waits and that no running
/// builds process is telling the judge's session of: one message for the
/// findings to settle of each landing, and one for each dispute to answer.
/// Marks each matter a message reached told. Returns a line for each message
/// sent, and one when the judge's inbox is gone, which leaves its matters
/// untold.
pub fn tell_waiting(
    store: &Store,
    stamp: &Stamp,
    sessions: &crate::inbox::Sessions,
) -> Result<Vec<String>> {
    let conn = store.open_existing()?;
    let Some(judge) = judge::listening(&conn, sessions)? else {
        return Ok(Vec::new());
    };
    let waiting = untold(&conn, &judge)?;
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
                "builds told the judge {} of findings {} of review landing #{landing}, which no running builds process was telling it of",
                judge.by,
                to_settle.ids()
            );
            notices.push((notice, line));
        }
    }
    for waited in waiting.iter().filter(|w| w.matter == Matter::Answer) {
        let notice = judge::dispute_notice(&conn, &store.project, &waited.finding)?;
        let line = format!(
            "builds told the judge {} of the dispute of finding {}, which no running builds process was telling it of",
            judge.by, waited.finding.id
        );
        notices.push((notice, line));
    }
    let mut lines = Vec::new();
    for (notice, line) in notices {
        match judge::tell_judge(sessions, &judge, &notice)? {
            accepted @ (Sent::Delivered | Sent::Queued) => {
                record(store, stamp, &notice.matters, &judge.session)?;
                lines.push(
                    format!("{line}. {}", accepted.resume_notice())
                        .trim_end()
                        .to_string(),
                );
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
pub fn forget_ended(
    store: &Store,
    stamp: &Stamp,
    sessions: &crate::inbox::Sessions,
) -> Result<Option<String>> {
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
        "the session of project {}'s judge {} ended, so builds forgot it{}",
        store.project,
        judge.by,
        if waiting.is_empty() {
            String::new()
        } else {
            format!(": findings {} wait", waiting.join(", "))
        }
    )))
}

/// Makes every row name a process that no longer runs: the processes that
/// wrote them ended. For a test of a matter that waits after its writer ended.
#[cfg(test)]
pub fn end_writers(store: &Store) {
    rewrite_writers(store, "writer_pid = 0, writer_started = 0");
}

/// Makes every row name the start time of another process than the one with
/// its pid, as when the system gives the writer's pid to a later process.
#[cfg(test)]
pub fn reuse_writers_pids(store: &Store) {
    rewrite_writers(store, "writer_started = writer_started + 1000");
}

#[cfg(test)]
fn rewrite_writers(store: &Store, set: &str) {
    let stamp = Stamp {
        at: 0,
        by: "test".to_string(),
    };
    store
        .write(&stamp, "rewrite writers", |db| {
            db.execute(&format!("UPDATE review_judge_told SET {set}"), [])
                .sql()?;
            Ok(())
        })
        .unwrap();
}

#[cfg(test)]
#[path = "judge_told_tests.rs"]
mod tests;

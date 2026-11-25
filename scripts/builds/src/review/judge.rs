//! The judge of a project's reviews: a standing Codex or Claude Code session that
//! settles the findings of every review of the project's branches, so that
//! each finding is judged by a session that keeps the rules and the earlier
//! settles in its context.
//!
//! ```text
//! builds judge listen --project P [--by WHO]
//! builds review wait LANDING [--project P]
//! builds review dispute FINDING --why TEXT
//! builds review keep FINDING --why TEXT
//! ```
//!
//! `listen` keeps the inbox of the session that runs it (inbox.rs) as the
//! project's judge, in place of any earlier one. Claude Code's running
//! sessions (claude_sessions.rs) and Codex's resumable threads determine
//! whether the judge can receive a message. Only a main session judges.
//! A dispatched agent works on a branch the judge may need to settle.
//!
//! An implementing agent reviews its own branch. When a review leaves
//! findings that nobody has settled, `builds review` tells the judge of them
//! (`tell`), or, when no judge listens, the coordinator: the session that
//! dispatched the agent, whose inbox the agent shares. builds records the
//! dispatched agent that ran each review run as the agent of the branch it
//! read, which never settles that run's findings, and while a judge listens,
//! only the judge settles a finding (`may_judge`). The agent then runs
//! `builds review wait` as a background task, which
//! ends once every finding is settled (`wait`): no inbox reaches a
//! dispatched agent, and a background task's end does. Each settle tells the
//! waits through a folder of the store that a wait watches (`tell_waits`).
//!
//! When someone thinks the judge confirmed a finding wrongly, they dispute it
//! once (`dispute`): builds tells the judge, or the coordinator, as for a
//! review, and the wait ends once the judge answers, by settling the finding
//! again as dropped or by keeping it (`keep`). A second disagreement goes to
//! the coordinator, who asks the person whether to allow the finding to land.
//! The tables are `judge.sql`; this is the only code that writes them.
//!
//! builds records each matter it told a judge's session of (`judge_told.rs`),
//! and sends a judge that listens every matter it has not told that session
//! of: when the session starts listening, and after each settle, dispute or
//! answer, whichever put the matter there. A matter whose builds process
//! still runs is left to that process, which tells the judge itself;
//! `builds health` sends the others again.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, params};

use crate::ack::agent::Caller;
use crate::db::Sql;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::inbox::{Messaging, Recipient, Sent, StoredEndpoint};
use crate::lists::Stamp;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::Store;
use crate::review::judge_told::{self, Matter};
use crate::review::record::{self, Finding};
use crate::review::render;
use crate::tool::Tool;

const USAGE: &str = "usage: builds judge listen --project P [--by WHO]";

/// The session that judges a project's reviews.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judge {
    /// Its session id, which is its main agent's id too.
    pub session: String,
    /// The name it listened under, which its settles carry.
    pub by: String,
    pub inbox: Messaging,
}

impl Judge {
    /// Whether <caller> is this judge: the main agent of its session.
    pub fn is(&self, caller: &Caller) -> bool {
        caller.session == self.session && !caller.is_dispatched()
    }
}

/// The session that last listened as the project's judge, whether or not it
/// still runs.
fn last(db: &Connection) -> Result<Option<Judge>> {
    let row = db
        .query_row(
            "SELECT runtime, address, token, session, by FROM review_judge",
            [],
            |row| {
                Ok((
                    StoredEndpoint::read(row)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .sql()?;
    row.map(|(inbox, session, by)| {
        Ok(Judge {
            session,
            by,
            inbox: inbox.restore()?,
        })
    })
    .transpose()
}

/// The project's registered judge, while its Claude Code session is among
/// <running> (`claude_sessions::running_now`) or its Codex thread can be
/// resumed.
pub fn listening(
    db: &Connection,
    sessions: &crate::inbox::Sessions,
    running: &[crate::claude_sessions::Running],
) -> Result<Option<Judge>> {
    let Some(judge) = last(db)? else {
        return Ok(None);
    };
    let available = match &judge.inbox {
        Messaging::Claude(_) => running
            .iter()
            .any(|session| session.session_id == judge.session),
        Messaging::Codex(thread) => sessions.codex_can_resume(thread)?,
    };
    Ok(available.then_some(judge))
}

/// Records the caller as the agent of the reviewed branch, including a main
/// session that ran a review on its own branch.
pub fn record_branch_agent(db: &Connection, run: i64, caller: &Caller) -> Result<()> {
    db.execute(
        "INSERT INTO review_branch_agent (run_id, session, agent) VALUES (?1, ?2, ?3)",
        params![run, caller.session, caller.id],
    )
    .sql()?;
    Ok(())
}

/// The agent of the branch that review run <run> read, when a session
/// dispatched the agent that ran it.
pub fn branch_agent(db: &Connection, run: i64) -> Result<Option<Caller>> {
    db.query_row(
        "SELECT session, agent FROM review_branch_agent WHERE run_id = ?1",
        [run],
        |row| {
            Ok(Caller {
                session: row.get(0)?,
                id: row.get(1)?,
            })
        },
    )
    .optional()
    .sql()
}

/// Whether <owner>, the agent that ran a review, is the one that would settle
/// its findings: the project's <judge> while one listens, else a session's
/// main agent, the coordinator. Such a review needs an independent settler.
pub fn would_settle(judge: Option<&Judge>, owner: &Caller) -> bool {
    match judge {
        Some(judge) => judge.is(owner),
        None => !owner.is_dispatched(),
    }
}

/// Whether <finding>'s review belongs to an agent that matches <is>.
pub fn owner_matches(
    db: &Connection,
    finding: &Finding,
    is: impl FnOnce(&Caller) -> bool,
) -> Result<bool> {
    Ok(branch_agent(db, finding.run_id)?.as_ref().is_some_and(is))
}

/// Refuses <caller> a settle of finding <id> that records no verdict, such as
/// the person's words that allow a confirmed finding to land, when <caller>
/// is the agent of the branch that holds the finding: the agent that ran the
/// review run that raised it. The person (no caller) is never refused.
pub fn may_record(db: &Connection, id: i64, caller: Option<&Caller>) -> Result<()> {
    let Some(caller) = caller else {
        return Ok(());
    };
    let finding = record::finding_of(db, id)?;
    if owner_matches(db, &finding, |owner| owner == caller)? {
        fail!(
            "you ran review run {}, which raised finding {id} on your branch, so you never settle it: the judge settles it, or the coordinator when no judge listens",
            finding.run_id
        );
    }
    Ok(())
}

/// Refuses <caller> a verdict on finding <id> (confirmed, dropped, or dropped
/// on a second settle) as `may_record` does, and, while a judge listens
/// (`listening`, with Claude Code's <running> sessions), unless
/// <caller> is the judge. The person (no caller) is never refused.
pub fn may_judge(
    db: &Connection,
    sessions: &crate::inbox::Sessions,
    running: &[crate::claude_sessions::Running],
    id: i64,
    caller: Option<&Caller>,
) -> Result<()> {
    may_record(db, id, caller)?;
    let Some(caller) = caller else {
        return Ok(());
    };
    match listening(db, sessions, running)? {
        Some(judge) if !judge.is(caller) => fail!(
            "{} judges this project's reviews while its session runs, and only the judge settles a finding: leave finding {id} to it",
            judge.by
        ),
        _ => Ok(()),
    }
}

/// The findings of a review that nobody has settled, and the review they
/// belong to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToSettle {
    pub project: String,
    pub landing: i64,
    /// The key of the change under review, such as `branch:work`.
    pub key: String,
    /// The worktree that holds the branch, when the review that raised the
    /// findings is the one that tells of them.
    pub worktree: Option<PathBuf>,
    pub findings: Vec<i64>,
}

impl ToSettle {
    pub(super) fn ids(&self) -> String {
        self.findings
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The session builds tells of a review's findings: the judge, or, with no
/// judge listening, the coordinator of the dispatched agent that runs the
/// command.
enum Settler<'a> {
    Judge(&'a Judge),
    Coordinator(&'a Caller),
}

impl Settler<'_> {
    /// The message's opening, about <project>: to the coordinator, it says
    /// that its agent <did> something.
    fn opening(&self, project: &str, did: &str) -> String {
        match self {
            Settler::Judge(_) => format!("You judge project {project}"),
            Settler::Coordinator(agent) => format!(
                "No judge listens for project {project}, and your agent {} {did}",
                agent.id
            ),
        }
    }

    /// The `--by` that the settler's commands carry.
    fn by(&self) -> &str {
        match self {
            Settler::Judge(judge) => &judge.by,
            Settler::Coordinator(_) => "WHO",
        }
    }
}

/// What an agent tells the session that settles: the findings of a review
/// to settle, or a dispute of a confirmed finding to answer.
pub struct Notice {
    project: String,
    landing: i64,
    /// What the agent did, as the coordinator's message says it.
    did: &'static str,
    /// The message after its opening, given the `--by` that the settler's
    /// commands carry.
    message: Box<dyn Fn(&str) -> String>,
    /// What builds told the settler, as the agent reads it: "to settle
    /// findings 3, 4".
    asked: String,
    /// The matters the notice puts to the judge: each finding to settle, or
    /// each dispute to answer.
    pub matters: Vec<(i64, Matter)>,
    /// What the agent's wait ends on and prints.
    wait_ends: &'static str,
    /// What the agent does when it settles findings itself.
    yourself: String,
}

impl ToSettle {
    /// The notice of these findings; None when there are none.
    pub fn notice(&self) -> Option<Notice> {
        if self.findings.is_empty() {
            return None;
        }
        let to = self.clone();
        Some(Notice {
            project: self.project.clone(),
            landing: self.landing,
            did: "ran a review",
            message: Box::new(move |by| {
                let place = match &to.worktree {
                    Some(worktree) => format!("{} in {}", to.key, worktree.display()),
                    None => to.key.clone(),
                };
                format!(
                    "findings {} of review landing #{} of project {} ({place}) wait for you to settle them. List them with `builds review show {} --project {}`, judge each against the code and its rule, and settle each with `builds review settle N confirmed|dropped --why TEXT --project {} --by {by}`.",
                    to.ids(),
                    to.landing,
                    to.project,
                    to.landing,
                    to.project,
                    to.project,
                )
            }),
            matters: self
                .findings
                .iter()
                .map(|id| (*id, Matter::Settle))
                .collect(),
            asked: format!("to settle findings {}", self.ids()),
            wait_ends: "the review has no finding left to settle, and prints each settle with its reason",
            yourself: format!(
                "settle findings {} yourself with `builds review settle N confirmed|dropped --why TEXT`.",
                self.ids()
            ),
        })
    }
}

/// The notice of the dispute of <finding> of <project>. Fails when nobody
/// disputed it, and when it belongs to no review landing.
pub fn dispute_notice(db: &Connection, project: &str, finding: &Finding) -> Result<Notice> {
    let (id, landing) = (finding.id, landing_of(finding)?);
    let Some(dispute) = dispute_of(db, id)? else {
        fail!("nobody disputed finding {id}");
    };
    let (file, line) = finding.location();
    let (project_owned, rule, file) = (project.to_string(), finding.rule.clone(), file.to_string());
    Ok(Notice {
        project: project.to_string(),
        landing,
        did: "disputes a finding",
        message: Box::new(move |by| {
            let project = &project_owned;
            format!(
                "{} thinks confirmed finding {id} of review landing #{landing} ({rule}, {file}:{line}) is wrong: {}. Answer once: settle it again as dropped with `builds review settle {id} --again --why TEXT --project {project} --by {by}`, or keep it with `builds review keep {id} --why TEXT --project {project} --by {by}`.",
                dispute.by,
                crate::text::one_line(&dispute.why),
            )
        }),
        matters: vec![(id, Matter::Answer)],
        asked: format!("of your dispute of finding {id}"),
        wait_ends: "the dispute has its answer, and prints the answer",
        yourself: format!(
            "you settle findings yourself: settle finding {id} again as dropped, or keep it with `builds review keep {id} --why TEXT`."
        ),
    })
}

/// Sends <notice> to the eligible session that settles, and returns
/// what to tell <caller>, the agent that sent it: whom builds told, and
/// that it waits with `builds review wait`. A session's main agent or the
/// person, with no judge listening, settles itself.
pub fn tell(
    store: &Store,
    stamp: &Stamp,
    sessions: &crate::inbox::Sessions,
    running: &[crate::claude_sessions::Running],
    notice: &Notice,
    caller: Option<&Caller>,
    own: Option<&Messaging>,
) -> Result<String> {
    let project = &notice.project;
    let db = store.open_existing()?;
    let waits = format!(
        "Run `{}` as a background task: it ends once {}.\n",
        wait_command(project, notice.landing),
        notice.wait_ends
    );
    let asked = &notice.asked;
    if let Some(judge) = listening(&db, sessions, running)? {
        if has_owner(&db, notice, |owner| would_settle(Some(&judge), owner))? {
            return Ok(format!(
                "{} ran the review and cannot settle its findings. Ask an independent session to register as judge. {waits}",
                judge.by
            ));
        }
        return Ok(match tell_judge(sessions, &judge, notice)? {
            delivery @ (Sent::Delivered | Sent::Queued) => {
                judge_told::record(store, stamp, &notice.matters, &judge.session)?;
                format!(
                    "builds told the judge of project {project} ({}) {asked}. {}{waits}",
                    judge.by,
                    delivery.resume_notice()
                )
            }
            Sent::Gone => format!(
                "The judge of project {project} ({}) runs, but its inbox is gone, so builds told nobody, and only the judge settles while its session runs: tell the judge to run `builds judge listen --project {project} --by {}` again, which sends it {asked}, then {}",
                judge.by,
                judge.by,
                waits.replacen("Run", "run", 1)
            ),
        });
    }
    match caller {
        Some(caller) if caller.is_dispatched() => {
            if let Some(own) = own {
                let delivery = sessions.send(own, &notice.text(&Settler::Coordinator(caller)))?;
                if delivery != Sent::Gone {
                    return Ok(format!(
                        "No judge listens for project {project}, so builds told your coordinator {asked}. {}{waits}",
                        delivery.resume_notice()
                    ));
                }
            }
            Ok(format!(
                "No judge listens for project {project}, and nothing reads your session's inbox, so builds told nobody: tell your coordinator {asked}, then {}",
                waits.replacen("Run", "run", 1)
            ))
        }
        Some(caller) if has_owner(&db, notice, |owner| owner == caller)? => Ok(format!(
            "No judge listens for project {project}. Ask an independent session to settle the findings. {waits}"
        )),
        _ => Ok(format!(
            "No judge listens for project {project}: {}\n",
            notice.yourself
        )),
    }
}

fn has_owner(db: &Connection, notice: &Notice, is: impl Fn(&Caller) -> bool) -> Result<bool> {
    for (id, _) in &notice.matters {
        let finding = record::finding_of(db, *id)?;
        if owner_matches(db, &finding, &is)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Sends <notice> to <judge> alone, with no fallback to a coordinator.
pub(super) fn tell_judge(
    sessions: &crate::inbox::Sessions,
    judge: &Judge,
    notice: &Notice,
) -> Result<Sent> {
    sessions.send(&judge.inbox, &notice.text(&Settler::Judge(judge)))
}

impl Notice {
    /// The message to <settler>: the opening, then the notice's words.
    fn text(&self, settler: &Settler) -> String {
        format!(
            "{}: {}",
            settler.opening(&self.project, self.did),
            (self.message)(settler.by())
        )
    }
}

/// The command that waits for the settles of review landing <landing> of
/// <project>.
fn wait_command(project: &str, landing: i64) -> String {
    format!("builds review wait {landing} --project {project}")
}

/// A dispute of a confirmed finding: someone thinks the judge settled it
/// wrongly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dispute {
    pub why: String,
    pub by: String,
    /// The judge's answer that kept the finding confirmed: why, and who.
    pub kept: Option<(String, String)>,
}

/// The dispute of finding <id>, if anyone disputed it.
pub(super) fn dispute_of(db: &Connection, id: i64) -> Result<Option<Dispute>> {
    db.query_row(
        "SELECT d.why, d.by, k.why, k.by FROM review_dispute d LEFT JOIN review_kept k ON k.finding_id = d.finding_id WHERE d.finding_id = ?1",
        [id],
        |row| {
            let kept: (Option<String>, Option<String>) = (row.get(2)?, row.get(3)?);
            Ok(Dispute {
                why: row.get(0)?,
                by: row.get(1)?,
                kept: kept.0.zip(kept.1),
            })
        },
    )
    .optional()
    .sql()
}

/// The review landing of <finding>. Every review run belongs to a landing,
/// so a finding of a review with none is broken state, and this fails
/// naming the finding.
pub(super) fn landing_of(finding: &Finding) -> Result<i64> {
    match finding.landing_id {
        Some(landing) => Ok(landing),
        None => fail!(
            "finding {} of run {} belongs to no review landing, as no finding of a review can",
            finding.id,
            finding.run_id
        ),
    }
}

/// Whether the dispute of <finding> waits for an answer: the judge neither
/// kept the finding nor settled it again, and no review closed it.
fn waits_for_answer(finding: &Finding, dispute: &Dispute) -> bool {
    finding.is_open() && dispute.kept.is_none()
}

/// Records <stamp>'s dispute of open confirmed finding <id>, with <why>, and
/// returns the finding. A finding has one dispute: builds refuses a second,
/// and a finding that is no open confirmed finding of a review.
pub fn dispute(db: &Connection, id: i64, why: &str, stamp: &Stamp) -> Result<Finding> {
    let finding = record::finding_of(db, id)?;
    if !finding.is_open() {
        fail!(
            "finding {id} is no open confirmed finding of a review, so there is nothing to dispute"
        );
    }
    match dispute_of(db, id)? {
        Some(Dispute { kept: Some(_), .. }) => fail!(
            "the judge answered the dispute of finding {id} and kept it: a second disagreement goes to the coordinator, who asks the person whether to allow the finding to land as it is"
        ),
        Some(_) => fail!(
            "the dispute of finding {id} waits for its answer: run `builds review wait {}` as a background task",
            landing_of(&finding)?
        ),
        None => {}
    }
    db.execute(
        "INSERT INTO review_dispute (finding_id, why, at, by) VALUES (?1, ?2, ?3, ?4)",
        params![id, why, stamp.at, stamp.by],
    )
    .sql()?;
    judge_told::begin(db, &[(id, Matter::Answer)])?;
    Ok(finding)
}

/// Records the answer that keeps disputed finding <id> confirmed, with
/// <why>. Refuses a finding with no dispute that waits for an answer.
pub fn keep(db: &Connection, id: i64, why: &str, stamp: &Stamp) -> Result<()> {
    let finding = record::finding_of(db, id)?;
    let Some(dispute) = dispute_of(db, id)? else {
        fail!("nobody disputed finding {id}, so there is no dispute to answer");
    };
    if !waits_for_answer(&finding, &dispute) {
        fail!(
            "the dispute of finding {id} needs no answer: the finding was kept already, or it is no open confirmed finding"
        );
    }
    db.execute(
        "INSERT INTO review_kept (finding_id, why, at, by) VALUES (?1, ?2, ?3, ?4)",
        params![id, why, stamp.at, stamp.by],
    )
    .sql()?;
    Ok(())
}

/// How long `builds review wait` waits for the settles before it fails.
pub const WAIT_LIMIT: Duration = Duration::from_secs(2 * 60 * 60);

/// The folder in <store_dir> whose changes tell each `builds review wait`
/// that a finding was settled: a settle adds a file there and removes it.
fn settles_dir(store_dir: &Path) -> PathBuf {
    store_dir.join("review-settles")
}

/// Tells every `builds review wait` of the store at <store_dir> that a
/// finding was settled, after the settle's write committed: a wait woken
/// before the commit would read the finding unsettled and wait on. Fails
/// when the waits could not be told, since a wait that is not told ends
/// only at its limit; the settle stands, and the error says so.
pub fn tell_waits(store_dir: &Path) -> Result<()> {
    let dir = settles_dir(store_dir);
    if !dir.is_dir() {
        return Ok(());
    }
    let mark = dir.join(std::process::id().to_string());
    match std::fs::write(&mark, "").and_then(|()| std::fs::remove_file(&mark)) {
        Ok(()) => Ok(()),
        Err(error) => fail!(
            "builds recorded the settle, but could not tell the waits in {} of it: {error}. A `builds review wait` of this review ends only at its limit: stop it and run it again",
            dir.display()
        ),
    }
}

/// The matters of review landing <landing> that wait on the judge: each
/// finding nobody has settled, and each finding whose dispute waits for an
/// answer, with when the matter began to wait.
pub(super) fn waiting_matters(db: &Connection, landing: i64) -> Result<Vec<Waiting>> {
    let mut found = Vec::new();
    for finding in record::findings_of_landing(db, landing)? {
        if finding.is_unsettled() {
            found.push(Waiting {
                finding,
                matter: Matter::Settle,
            });
        } else if let Some(dispute) = dispute_of(db, finding.id)?
            && waits_for_answer(&finding, &dispute)
        {
            found.push(Waiting {
                finding,
                matter: Matter::Answer,
            });
        }
    }
    Ok(found)
}

/// A finding that waits on the judge, and why.
pub(super) struct Waiting {
    pub finding: Finding,
    pub matter: Matter,
}

/// The findings of review landing <landing> that wait on the judge.
fn waiting(db: &Connection, landing: i64) -> Result<Vec<Finding>> {
    Ok(waiting_matters(db, landing)?
        .into_iter()
        .map(|waiting| waiting.finding)
        .collect())
}

/// Waits, for at most <limit>, until every finding of review landing
/// <landing> in <store> is settled, and returns how the findings that
/// waited were settled and what is left to do. Fails when the limit ends
/// first.
pub fn wait(store: &Store, landing: i64, limit: Duration) -> Result<String> {
    let dir = settles_dir(&store.dir);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let (told, heard) = mpsc::channel();
    // Started before the first read: a settle after the read is heard.
    let _watch = crate::daemon::vnode::Watch::start(&dir, move || {
        let _ = told.send(());
    })
    .context(|| format!("watching {}", dir.display()))?;
    let ends = Instant::now() + limit;
    let waited: Vec<i64> = {
        let conn = store.open_existing()?;
        record::landing(&conn, landing)?;
        waiting(&conn, landing)?.iter().map(|f| f.id).collect()
    };
    loop {
        {
            let conn = store.open_existing()?;
            let left = waiting(&conn, landing)?;
            if left.is_empty() {
                return settled_words(&conn, landing, &waited);
            }
            let now = Instant::now();
            if now >= ends {
                fail!(
                    "after {} minutes, findings {} of review landing {landing} still wait for a settle or for the answer to a dispute; run `builds review wait {landing}` again to keep waiting",
                    limit.as_secs() / 60,
                    left.iter()
                        .map(|f| f.id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        // Heard a settle, or reached the limit: either way, read again.
        match heard.recv_timeout(ends.saturating_duration_since(Instant::now())) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                fail!("the watch of {} stopped", dir.display())
            }
        }
    }
}

/// How each of <waited>, findings of <landing>, was settled, and what is
/// left to do.
fn settled_words(db: &Connection, landing: i64, waited: &[i64]) -> Result<String> {
    let findings = record::findings_of_landing(db, landing)?;
    let mut text = format!(
        "review landing {landing} has no finding left to settle and no dispute left to answer\n"
    );
    for finding in findings.iter().filter(|f| waited.contains(&f.id)) {
        text.push_str(&render::finding_text(finding));
        if let Some(dispute) = dispute_of(db, finding.id)? {
            text.push_str(&format!(
                "    disputed by {}: {}\n",
                dispute.by,
                crate::text::one_line(&dispute.why)
            ));
            if let Some((why, by)) = &dispute.kept {
                text.push_str(&format!(
                    "    kept by {by} after the dispute: {}\n",
                    crate::text::one_line(why)
                ));
            }
        }
    }
    let open: Vec<&Finding> = findings.iter().filter(|f| f.is_open()).collect();
    if open.is_empty() {
        text.push_str("No confirmed finding is open.\n");
    } else {
        text.push_str(&format!(
            "{} open: fix each in a new commit, then run builds review in the branch's worktree, which closes each finding the fix removes. When you think one is wrong, dispute it once: builds review dispute <finding> --why TEXT\n",
            match open.len() {
                1 => "1 confirmed finding is".to_string(),
                n => format!("{n} confirmed findings are"),
            }
        ));
        for finding in open.iter().filter(|f| !waited.contains(&f.id)) {
            text.push_str(&render::finding_text(finding));
        }
    }
    Ok(text)
}

/// Keeps <inbox>, the inbox of <caller>'s session, as the project's judge,
/// in place of any earlier one. Refuses an agent that a session dispatched.
pub fn listen(db: &Connection, inbox: &Messaging, caller: &Caller, stamp: &Stamp) -> Result<()> {
    if caller.is_dispatched() {
        fail!(
            "session {} dispatched agent {}, which shares that session's inbox, so builds could not reach the agent: run builds judge listen as the main agent of the judge's own session",
            caller.session,
            caller.id
        );
    }
    let (runtime, address, token) = inbox.stored();
    db.execute(
        "INSERT OR REPLACE INTO review_judge (singleton, runtime, address, token, session, at, by) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            runtime,
            address,
            token,
            caller.session,
            stamp.at,
            stamp.by
        ],
    )
    .sql()?;
    Ok(())
}

pub fn move_legacy_judge(db: &Connection) -> Result<()> {
    use crate::db::{self, SCHEMA};
    crate::inbox::move_legacy_destination(
        db,
        "review_judge",
        db::statement(SCHEMA, "CREATE TABLE review_judge (", ") STRICT;"),
        "singleton",
        ", session, at, by",
    )?;
    db::remake_table(
        db,
        "review_branch_agent",
        db::statement(SCHEMA, "CREATE TABLE review_branch_agent (", ") STRICT;"),
        |db| {
            db.execute_batch("INSERT INTO review_branch_agent (run_id, session, agent) SELECT run_id, session, agent FROM review_branch_agent_old;").sql()
        },
        &[],
    )
}

/// `builds judge <command>`.
pub fn cmd_judge(tool: &Tool, args: &[String]) -> Result<()> {
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "listen" => {
            let said = listen_command(
                tool,
                rest,
                &current_dir()?,
                tool.session.caller()?.as_ref(),
                Messaging::for_session(&tool.session, Recipient::ThisThread)?.as_ref(),
            )?;
            print!("{said}");
            Ok(())
        }
        other => fail!("unknown judge command {other}\n{USAGE}"),
    }
}

/// `listen --project P`, run in <cwd> by <caller>, whose session's inbox is
/// <inbox>.
pub fn listen_command(
    tool: &Tool,
    args: &[String],
    cwd: &Path,
    caller: Option<&Caller>,
    inbox: Option<&Messaging>,
) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, USAGE)?;
    let Some(project) = args.value("project")? else {
        fail!("say which project's reviews the session judges with --project");
    };
    let Some(caller) = caller else {
        fail!(
            "Run this command from the Codex or Claude Code session that will judge the project."
        );
    };
    let Some(inbox) = inbox else {
        fail!(
            "This session cannot receive notifications. Run the command from a Codex thread or a Claude Code session with cross-session messaging."
        );
    };
    let store = Store::named(&tool.places, project)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, "judge listen", |db| {
        listen(db, inbox, caller, &stamp)
    })?;
    let mut said = format!(
        "This session judges project {project}. Builds sends it each review finding to settle.\n"
    );
    let sessions = tool.places.sessions();
    let running = crate::claude_sessions::running_now(&sessions.claude)
        .map_err(|why| Fail::Message(format!("reading which Claude Code sessions run: {why}")))?;
    for line in judge_told::tell_waiting(&store, &stamp, &sessions, &running)? {
        said.push_str(&line);
        said.push('\n');
    }
    Ok(said)
}

/// Forgets the project's judge, whose session ended, so that the coordinator
/// settles the findings that wait.
pub fn forget(db: &Connection) -> Result<()> {
    db.execute("DELETE FROM review_judge", []).sql()?;
    Ok(())
}

/// The judge that last listened, when its session no longer runs (`last`,
/// `listening`).
pub(super) fn ended(
    db: &Connection,
    sessions: &crate::inbox::Sessions,
    running: &[crate::claude_sessions::Running],
) -> Result<Option<Judge>> {
    Ok(match last(db)? {
        Some(judge) if listening(db, sessions, running)?.is_none() => Some(judge),
        _ => None,
    })
}

#[cfg(test)]
pub(crate) use tests::REVIEW_BRANCH_AGENT;

#[cfg(test)]
#[path = "judge_tests.rs"]
pub(crate) mod tests;

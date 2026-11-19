//! The judge of a project's reviews: a standing Claude Code session that
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
//! project's judge, in place of any earlier one. A judge listens while its
//! session runs, which Claude Code's record of its running sessions says
//! (claude_sessions.rs). Only a session's main agent judges: an agent that a
//! session dispatched shares that session's inbox, so a message to it would
//! reach its session instead.
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

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, params};

use crate::ack::agent::Caller;
use crate::db::Sql;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::inbox::{Messaging, Sent, send};
use crate::lists::Stamp;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::Store;
use crate::review::record::{self, Finding};
use crate::review::render;
use crate::tool::Tool;

const USAGE: &str = "usage: builds judge listen --project P [--by WHO]";

/// The session that judges a project's reviews.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judge {
    /// Its Claude Code session id, which is its main agent's id too.
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
    let row: Option<(String, Option<String>, String, String)> = db
        .query_row(
            "SELECT socket, token, session, by FROM review_judge",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .sql()?;
    row.map(|(socket, token, session, by)| {
        Ok(Judge {
            session,
            by,
            inbox: Messaging::new(&socket, token.as_deref())?,
        })
    })
    .transpose()
}

/// The project's judge, when the session that last listened as it still
/// runs: one of the running sessions that Claude Code records in
/// <sessions>.
pub fn listening(db: &Connection, sessions: &Path) -> Result<Option<Judge>> {
    let Some(judge) = last(db)? else {
        return Ok(None);
    };
    let running = crate::claude_sessions::running(sessions).map_err(|why| {
        Fail::Message(format!(
            "reading which Claude Code sessions run, to learn whether the judge's session runs: {why}"
        ))
    })?;
    Ok(running
        .iter()
        .any(|session| session.session_id == judge.session)
        .then_some(judge))
}

/// Records <caller> as the agent of the branch that review run <run> read,
/// when a session dispatched it; a session's main agent works on no branch
/// of its own, so builds records nothing for it.
pub fn record_branch_agent(db: &Connection, run: i64, caller: &Caller) -> Result<()> {
    if !caller.is_dispatched() {
        return Ok(());
    }
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

/// Refuses <caller> a settle of finding <id> that records no verdict, such as
/// the person's words that allow a confirmed finding to land, when <caller>
/// is the agent of the branch that holds the finding: the agent that ran the
/// review run that raised it. The person (no caller) is never refused.
pub fn may_record(db: &Connection, id: i64, caller: Option<&Caller>) -> Result<()> {
    let Some(caller) = caller else {
        return Ok(());
    };
    let finding = record::finding_of(db, id)?;
    if branch_agent(db, finding.run_id)?.as_ref() == Some(caller) {
        fail!(
            "you ran review run {}, which raised finding {id} on your branch, so you never settle it: the judge settles it, or the coordinator when no judge listens",
            finding.run_id
        );
    }
    Ok(())
}

/// Refuses <caller> a verdict on finding <id> (confirmed, dropped, or dropped
/// on a second settle) as `may_record` does, and, while a judge listens
/// (`listening`, with Claude Code's running sessions in <sessions>), unless
/// <caller> is the judge. The person (no caller) is never refused.
pub fn may_judge(db: &Connection, sessions: &Path, id: i64, caller: Option<&Caller>) -> Result<()> {
    may_record(db, id, caller)?;
    let Some(caller) = caller else {
        return Ok(());
    };
    match listening(db, sessions)? {
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
    /// The worktree that holds the branch.
    pub worktree: PathBuf,
    pub findings: Vec<i64>,
}

impl ToSettle {
    fn ids(&self) -> String {
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

/// Whom a message reached.
enum Told {
    /// The judge, named as it listened.
    Judge(String),
    Coordinator,
    /// No judge listens, and <caller> is the person or a session's main
    /// agent, which settles the findings itself: nothing was sent.
    Caller,
    /// No judge listens, and nothing reads the dispatched agent's session's
    /// inbox.
    Nobody,
}

/// Sends <message> for the session that settles, once: the judge when one
/// listens (`listening`, with Claude Code's running sessions in
/// <sessions>); else, when a session dispatched <caller>, the coordinator,
/// which is <caller>'s own session and reads its inbox <own>. A send whose
/// session ended goes on to the next; any other failure of a send is the
/// error.
fn tell_settler(
    db: &Connection,
    sessions: &Path,
    caller: Option<&Caller>,
    own: Option<&Messaging>,
    message: impl Fn(&Settler) -> String,
) -> Result<Told> {
    if let Some(judge) = listening(db, sessions)?
        && send(&judge.inbox, &message(&Settler::Judge(&judge)))? == Sent::Delivered
    {
        return Ok(Told::Judge(judge.by));
    }
    let Some(caller) = caller.filter(|caller| caller.is_dispatched()) else {
        return Ok(Told::Caller);
    };
    let Some(own) = own else {
        return Ok(Told::Nobody);
    };
    Ok(match send(own, &message(&Settler::Coordinator(caller)))? {
        Sent::Delivered => Told::Coordinator,
        Sent::Gone => Told::Nobody,
    })
}

/// Tells the session that settles <to>'s findings of them (`tell_settler`),
/// and returns what to tell <caller>, the agent that ran the review: whom
/// builds told, and that it waits for the settles with `builds review
/// wait`. A session's main agent or the person, with no judge listening,
/// settles the findings itself.
pub fn tell(
    db: &Connection,
    sessions: &Path,
    to: &ToSettle,
    caller: Option<&Caller>,
    own: Option<&Messaging>,
) -> Result<String> {
    if to.findings.is_empty() {
        return Ok(String::new());
    }
    let told = tell_settler(db, sessions, caller, own, |settler| {
        format!(
            "{}: findings {} of review landing #{} of project {} ({} in {}) wait for you to settle them. List them with `builds review show {} --project {}`, judge each against the code and its rule, and settle each with `builds review settle N confirmed|dropped --why TEXT --project {} --by {}`.",
            settler.opening(&to.project, "ran a review"),
            to.ids(),
            to.landing,
            to.project,
            to.key,
            to.worktree.display(),
            to.landing,
            to.project,
            to.project,
            settler.by(),
        )
    })?;
    let waits = format!(
        "Run `{}` as a background task: it ends once the review has no finding left to settle, and prints each settle with its reason.\n",
        wait_command(&to.project, to.landing)
    );
    let (project, ids) = (&to.project, to.ids());
    Ok(match told {
        Told::Judge(by) => {
            format!(
                "builds told the judge of project {project} ({by}) to settle findings {ids}. {waits}"
            )
        }
        Told::Coordinator => format!(
            "No judge listens for project {project}, so builds told your coordinator to settle findings {ids}. {waits}"
        ),
        Told::Caller => format!(
            "No judge listens for project {project}: settle findings {ids} yourself with `builds review settle N confirmed|dropped --why TEXT`.\n"
        ),
        Told::Nobody => format!(
            "No judge listens for project {project}, and nothing reads your session's inbox, so builds told nobody: tell your coordinator to settle findings {ids}, then {}",
            waits.replacen("Run", "run", 1)
        ),
    })
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
fn dispute_of(db: &Connection, id: i64) -> Result<Option<Dispute>> {
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
fn landing_of(finding: &Finding) -> Result<i64> {
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
    if why.trim().is_empty() {
        fail!("--why says nothing");
    }
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
    Ok(finding)
}

/// Records the answer that keeps disputed finding <id> confirmed, with
/// <why>. Refuses a finding with no dispute that waits for an answer.
pub fn keep(db: &Connection, id: i64, why: &str, stamp: &Stamp) -> Result<()> {
    if why.trim().is_empty() {
        fail!("--why says nothing");
    }
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

/// Tells the session that settles of the dispute of <finding> of
/// <project> (`tell_settler`), and returns what to tell <caller>, who
/// disputed it: whom builds told, and that it waits for the answer with
/// `builds review wait`.
pub fn tell_dispute(
    db: &Connection,
    sessions: &Path,
    project: &str,
    finding: &Finding,
    caller: Option<&Caller>,
    own: Option<&Messaging>,
) -> Result<String> {
    let (id, landing) = (finding.id, landing_of(finding)?);
    let Some(dispute) = dispute_of(db, id)? else {
        fail!("nobody disputed finding {id}");
    };
    let told = tell_settler(db, sessions, caller, own, |settler| {
        let by = settler.by();
        format!(
            "{}: {} thinks confirmed finding {id} of review landing #{landing} ({}, {}:{}) is wrong: {}. Answer once: settle it again as dropped with `builds review settle {id} --again --why TEXT --project {project} --by {by}`, or keep it with `builds review keep {id} --why TEXT --project {project} --by {by}`.",
            settler.opening(project, "disputes a finding"),
            dispute.by,
            finding.rule,
            finding.file,
            finding.line,
            crate::text::one_line(&dispute.why),
        )
    })?;
    let waits = format!(
        "Run `{}` as a background task: it ends once the dispute has its answer, and prints the answer.\n",
        wait_command(project, landing)
    );
    Ok(match told {
        Told::Judge(by) => {
            format!(
                "builds told the judge of project {project} ({by}) of your dispute of finding {id}. {waits}"
            )
        }
        Told::Coordinator => format!(
            "No judge listens for project {project}, so builds told your coordinator of your dispute of finding {id}. {waits}"
        ),
        Told::Caller => format!(
            "No judge listens for project {project}, and you settle findings yourself: settle finding {id} again as dropped, or keep it with `builds review keep {id} --why TEXT`.\n"
        ),
        Told::Nobody => format!(
            "No judge listens for project {project}, and nothing reads your session's inbox, so builds told nobody: tell your coordinator of your dispute of finding {id}, then {}",
            waits.replacen("Run", "run", 1)
        ),
    })
}

/// How long `builds review wait` waits for the settles before it fails.
pub const WAIT_LIMIT: Duration = Duration::from_secs(2 * 60 * 60);

/// The folder in <store_dir> whose changes tell each `builds review wait`
/// that a finding was settled: a settle adds a file there and removes it.
fn settles_dir(store_dir: &Path) -> PathBuf {
    store_dir.join("review-settles")
}

/// Tells every `builds review wait` of the store at <store_dir> that a
/// finding was settled, after the settle's write. A wait that misses it
/// reads the findings again when its limit ends, so a failure here only
/// makes a wait end late; it goes to stderr.
pub fn tell_waits(store_dir: &Path) {
    let dir = settles_dir(store_dir);
    if !dir.is_dir() {
        return;
    }
    let mark = dir.join(std::process::id().to_string());
    let made = std::fs::write(&mark, "").and_then(|()| std::fs::remove_file(&mark));
    if let Err(error) = made {
        crate::text::note(format!(
            "telling the waits in {} of a settle: {error}",
            dir.display()
        ));
    }
}

/// The findings of review landing <landing> that wait on the judge: those
/// nobody has settled, and those whose dispute waits for an answer.
fn waiting(db: &Connection, landing: i64) -> Result<Vec<Finding>> {
    let mut found = Vec::new();
    for finding in record::findings_of_landing(db, landing)? {
        let disputed = dispute_of(db, finding.id)?;
        if finding.is_unsettled()
            || disputed.is_some_and(|dispute| waits_for_answer(&finding, &dispute))
        {
            found.push(finding);
        }
    }
    Ok(found)
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
    db.execute(
        "INSERT OR REPLACE INTO review_judge (singleton, socket, token, session, at, by) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![
            inbox.socket(),
            inbox.token(),
            caller.session,
            stamp.at,
            stamp.by
        ],
    )
    .sql()?;
    Ok(())
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
                Caller::from_env()?.as_ref(),
                Messaging::from_env()?.as_ref(),
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
            "this command runs outside a Claude Code session (the environment has no CLAUDE_CODE_SESSION_ID), so builds could not reach the judge; run it from the judge's session"
        );
    };
    let Some(inbox) = inbox else {
        fail!(
            "this command runs in a Claude Code session with no inbox (the environment has no CLAUDE_CODE_MESSAGING_SOCKET), so builds could not reach the judge; run it from a session with cross-session messaging"
        );
    };
    let store = Store::named(&tool.places, project)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, "judge listen", |db| {
        listen(db, inbox, caller, &stamp)
    })?;
    Ok(format!(
        "this session is project {project}'s judge while it runs: builds tells it of each review's findings to settle, and only it settles them\n"
    ))
}

#[cfg(test)]
#[path = "judge_tests.rs"]
mod tests;

//! The judge of a project's reviews: a standing Claude Code session that
//! settles the findings of every review of the project's branches, so that
//! each finding is judged by a session that keeps the rules and the earlier
//! settles in its context.
//!
//! ```text
//! builds judge listen --project P [--by WHO]
//! builds review wait LANDING [--project P]
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
//! read. The agent then runs `builds review wait` as a background task, which
//! ends once every finding is settled (`wait`): no inbox reaches a
//! dispatched agent, and a background task's end does. Each settle tells the
//! waits through a folder of the store that a wait watches (`tell_waits`).
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

    /// What a session that settles the findings is told, after <opening>.
    fn words(&self, opening: &str, by: &str) -> String {
        format!(
            "{opening}: findings {} of review landing #{} of project {} ({} in {}) wait for you to settle them. List them with `builds review show {} --project {}`, judge each against the code and its rule, and settle each with `builds review settle N confirmed|dropped --why TEXT --project {} --by {by}`.",
            self.ids(),
            self.landing,
            self.project,
            self.key,
            self.worktree.display(),
            self.landing,
            self.project,
            self.project,
        )
    }

    /// The command that waits for the settles.
    fn wait_command(&self) -> String {
        format!(
            "builds review wait {} --project {}",
            self.landing, self.project
        )
    }
}

/// Tells the session that settles <to>'s findings of them, once, and
/// returns what to tell <caller>, the agent that ran the review: the judge
/// when one listens (`listening`, with Claude Code's running sessions in
/// <sessions>); else, when a session dispatched <caller>, the coordinator,
/// which is <caller>'s own session and reads its inbox <own>. A session's
/// main agent or the person, with no judge listening, settles the findings
/// itself, and builds tells nobody. A send whose session ended, or a
/// dispatched agent whose session has no inbox, tells nobody and says so; any
/// other failure of a send is the error.
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
    let waits = format!(
        "Run `{}` as a background task: it ends once the review has no finding left to settle, and prints each settle with its reason.\n",
        to.wait_command()
    );
    if let Some(judge) = listening(db, sessions)? {
        let words = to.words(&format!("You judge project {}", to.project), &judge.by);
        if send(&judge.inbox, &words)? == Sent::Delivered {
            return Ok(format!(
                "builds told the judge of project {} ({}) to settle findings {}. {waits}",
                to.project,
                judge.by,
                to.ids()
            ));
        }
    }
    let Some(caller) = caller.filter(|caller| caller.is_dispatched()) else {
        return Ok(format!(
            "No judge listens for project {}: settle findings {} yourself with `builds review settle N confirmed|dropped --why TEXT`.\n",
            to.project,
            to.ids()
        ));
    };
    let told = match own {
        Some(own) => send(
            own,
            &to.words(
                &format!(
                    "No judge listens for project {}, and your agent {} ran this review",
                    to.project, caller.id
                ),
                "WHO",
            ),
        )?,
        None => Sent::Gone,
    };
    Ok(match told {
        Sent::Delivered => format!(
            "No judge listens for project {}, so builds told your coordinator to settle findings {}. {waits}",
            to.project,
            to.ids()
        ),
        Sent::Gone => format!(
            "No judge listens for project {}, and nothing reads your session's inbox, so builds told nobody: tell your coordinator to settle findings {}, then {}",
            to.project,
            to.ids(),
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

/// The findings of review landing <landing> that nobody has settled.
fn unsettled(db: &Connection, landing: i64) -> Result<Vec<Finding>> {
    Ok(record::findings_of_landing(db, landing)?
        .into_iter()
        .filter(Finding::is_unsettled)
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
        unsettled(&conn, landing)?.iter().map(|f| f.id).collect()
    };
    loop {
        {
            let conn = store.open_existing()?;
            let left = unsettled(&conn, landing)?;
            if left.is_empty() {
                return settled_words(&conn, landing, &waited);
            }
            let now = Instant::now();
            if now >= ends {
                fail!(
                    "after {} minutes, nobody has settled findings {} of review landing {landing}; run `builds review wait {landing}` again to keep waiting",
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
    let mut text = format!("review landing {landing} has no finding left to settle\n");
    for finding in findings.iter().filter(|f| waited.contains(&f.id)) {
        text.push_str(&render::finding_text(finding));
    }
    let open: Vec<&Finding> = findings.iter().filter(|f| f.is_open()).collect();
    if open.is_empty() {
        text.push_str("No confirmed finding is open.\n");
    } else {
        text.push_str(&format!(
            "{} open: fix each in a new commit, then run builds review in the branch's worktree, which closes each finding the fix removes:\n",
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
        "this session is project {project}'s judge while it runs: builds tells it of each review's findings to settle\n"
    ))
}

#[cfg(test)]
#[path = "judge_tests.rs"]
mod tests;

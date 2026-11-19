//! The judge of a project's reviews: a standing Claude Code session, which
//! builds keeps by its inbox and its session id.
//!
//! ```text
//! builds judge listen --project P [--by WHO]
//! ```
//!
//! `listen` keeps the inbox of the session that runs it (inbox.rs) as the
//! project's judge, in place of any earlier one. Only a session's main agent
//! judges: an agent that a session dispatched shares that session's inbox, so
//! a message to it would reach its session instead. The table is `judge.sql`;
//! this is the only code that writes it.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::ack::agent::Caller;
use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::inbox::Messaging;
use crate::lists::Stamp;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::Store;
use crate::tool::Tool;

const USAGE: &str = "usage: builds judge listen --project P [--by WHO]";

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
        "this session is project {project}'s judge while it runs\n"
    ))
}

#[cfg(test)]
#[path = "judge_tests.rs"]
mod tests;

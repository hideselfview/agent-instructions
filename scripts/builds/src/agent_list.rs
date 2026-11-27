//! `builds agent list` and `builds agent show`: what the store records of the
//! agents `builds agent run` started, with whether each runs now (from
//! Claude Code's record of its running sessions, never from the store) and
//! how full its context is.

use std::io::Write;
use std::path::Path;

use crate::agent_launch::LOG_FILE;
use crate::agent_record::{self, Agent, Context, HANDOFF_TOKENS};
use crate::agent_stream::Line;
use crate::db;
use crate::fail;
use crate::fail::Result;
use crate::lists::args::Args;
use crate::lists::store::Store;
use crate::text::local_minute;
use crate::tool::Tool;

pub(crate) fn thousands(tokens: u64) -> String {
    format!("{}k", tokens / 1000)
}

/// The context of an agent's last turn against its model's window, such as
/// `292k of 1000k`, with a mark when it is past the handoff size.
pub fn context_words(context: Option<Context>) -> String {
    let Some(context) = context else {
        return "no turn read".to_string();
    };
    let mut words = match context.window {
        Some(window) => format!("{} of {}", thousands(context.tokens), thousands(window)),
        None => thousands(context.tokens),
    };
    if context.past_handoff() {
        words.push_str(&format!(
            ", past the handoff size of {}",
            thousands(HANDOFF_TOKENS)
        ));
    }
    words
}

/// Whether the agent runs now, or when its last process ended.
fn state_words(running: bool, last_end: Option<Option<(i64, i32)>>) -> String {
    match (running, last_end) {
        (true, _) => "running".to_string(),
        (false, Some(Some((at, _)))) => format!("ended {}", local_minute(at)),
        (false, _) => "ended, builds saw no end".to_string(),
    }
}

fn running_sessions(tool: &Tool) -> Result<Vec<String>> {
    Ok(
        crate::claude_sessions::running_now(&tool.places.claude_sessions())
            .map_err(|why| {
                crate::fail::Fail::Message(format!(
                    "reading the running Claude Code sessions: {why}"
                ))
            })?
            .into_iter()
            .map(|running| running.session_id)
            .collect(),
    )
}

fn items_words(agent: &Agent) -> String {
    agent
        .items
        .iter()
        .map(|item| format!("#{item}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `builds agent list`, printing to <out>.
pub fn list(tool: &Tool, args: &[String], cwd: &Path, out: &mut dyn Write) -> Result<()> {
    let args = Args::parse(args, &[], &["running"])?;
    args.exactly(
        0,
        &crate::command::Command::Agent(crate::command::agent::List).usage_text(),
    )?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let conn = db::open(&store.database(), db::Make::Never)?;
    let running = running_sessions(tool)?;
    for agent in agent_record::read_all(&conn)? {
        let runs = running.contains(&agent.session);
        if args.switch("running") && !runs {
            continue;
        }
        let last = agent_record::processes(&conn, &agent.session)?
            .last()
            .map(|row| row.ended);
        let _ = writeln!(
            out,
            "{}  {}  {}  {}  {}  {}  {}",
            agent.session,
            agent.work.role().word(),
            agent.model,
            items_words(&agent),
            agent.folder,
            state_words(runs, last),
            context_words(agent.context)
        );
    }
    Ok(())
}

/// The text of the last `result` line of the log at <path>, None when the
/// log has none.
fn last_result(path: &Path) -> Result<Option<String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("reading {}: {error}", path.display()),
    };
    let mut last = None;
    for line in text.lines() {
        if let Line::Result(report) = crate::agent_stream::parse(line)? {
            last = Some(report.text);
        }
    }
    Ok(last)
}

/// `builds agent show`, printing to <out>.
pub fn show(tool: &Tool, args: &[String], cwd: &Path, out: &mut dyn Write) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [session] = args.exactly(
        1,
        &crate::command::Command::Agent(crate::command::agent::Show).usage_text(),
    )?
    else {
        unreachable!("exactly one")
    };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let conn = db::open(&store.database(), db::Make::Never)?;
    let Some(agent) = agent_record::read(&conn, session)? else {
        fail!("project {} records no agent {session}", store.project);
    };
    let processes = agent_record::processes(&conn, session)?;
    let runs = running_sessions(tool)?.contains(session);
    let log = tool.places.agents().join(session).join(LOG_FILE);
    let mut say = |line: String| {
        let _ = writeln!(out, "{line}");
    };
    say(format!(
        "agent {session}: {} of project {}, {}",
        agent.work.role().word(),
        store.project,
        state_words(runs, processes.last().map(|row| row.ended))
    ));
    say(format!(
        "model: {} ({} tier)",
        agent.model,
        agent.tier.word()
    ));
    say(format!("items: {}", items_words(&agent)));
    say(format!("folder: {}", agent.folder));
    say(format!("brief: {}", agent.brief));
    for place in agent.work.places() {
        say(format!(
            "worktree: {} on branch {}",
            place.worktree, place.branch
        ));
    }
    say(format!(
        "started: {} by {}",
        local_minute(agent.started_at),
        agent.started_by
    ));
    say(format!("context: {}", context_words(agent.context)));
    say(format!("log: {}", log.display()));
    for row in &processes {
        let ended = match row.ended {
            Some((at, status)) => format!("ended {} with exit status {status}", local_minute(at)),
            None => "builds saw no end".to_string(),
        };
        let kernel_start = match row.process.pid_started {
            Some(at) => local_minute(at),
            None => "start unknown".to_string(),
        };
        say(format!(
            "process {}: pid {} ({kernel_start}), started by builds {}, message {}, {ended}",
            row.number,
            row.process.pid,
            local_minute(row.process.started_at),
            row.process.message
        ));
    }
    match last_result(&log)? {
        Some(text) => say(format!("last result:\n{text}")),
        None => say("last result: none in the log".to_string()),
    }
    Ok(())
}

#[cfg(test)]
#[path = "agent_list_tests.rs"]
mod tests;

//! The agents `builds agent run` starts, in the project's store
//! (agent_record.sql). This is the only code that writes those tables.

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::model_tier::Tier;

/// What an agent was started to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRole {
    /// Builds items to a brief, in a claimed worktree.
    Implementing,
    /// Writes a plan or briefs, and commits nothing.
    Planner,
}

impl AgentRole {
    /// The word `--role` takes and the store keeps.
    pub fn word(self) -> &'static str {
        match self {
            AgentRole::Implementing => "implementing",
            AgentRole::Planner => "planner",
        }
    }

    pub fn from_word(word: &str) -> Option<AgentRole> {
        [AgentRole::Implementing, AgentRole::Planner]
            .into_iter()
            .find(|role| role.word() == word)
    }
}

/// An agent as the store keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub session: String,
    pub role: AgentRole,
    pub model: String,
    pub tier: Tier,
    pub folder: String,
    pub brief: Option<String>,
    /// The worktrees and branches of an implementing agent's items, in the
    /// order of the repos. The first is the folder it runs in.
    pub places: Vec<(String, String)>,
    pub started_at: i64,
    pub started_by: String,
    pub items: Vec<u32>,
}

/// A process of an agent, as builds records its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: i32,
    pub pid_started: i64,
    pub started_at: i64,
    /// The hash of the message builds sent the process.
    pub message: String,
}

/// Records a new agent, its items and its first process, whole or not at
/// all. The agent's `places` hold one pair per repo; the table keeps the
/// first.
pub fn start(db: &Connection, agent: &Agent, process: &Process) -> Result<()> {
    let (worktree, branch) = match agent.places.first() {
        Some((worktree, branch)) => (Some(worktree), Some(branch)),
        None => (None, None),
    };
    db.execute(
        "INSERT INTO agent (session, role, model, tier, folder, brief, worktree, branch, started_at, started_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            agent.session,
            agent.role.word(),
            agent.model,
            agent.tier.word(),
            agent.folder,
            agent.brief,
            worktree,
            branch,
            agent.started_at,
            agent.started_by,
        ],
    )
    .sql()?;
    add_items(db, &agent.session, &agent.items)?;
    add_process(db, &agent.session, process)
}

/// Records that <items> are among the items of <session>'s agent.
pub fn add_items(db: &Connection, session: &str, items: &[u32]) -> Result<()> {
    for item in items {
        db.execute(
            "INSERT OR IGNORE INTO agent_item (session, item) VALUES (?1, ?2)",
            params![session, item],
        )
        .sql()?;
    }
    Ok(())
}

/// Records <process> as the next process of <session>'s agent. Its number is
/// the one after the last.
pub fn add_process(db: &Connection, session: &str, process: &Process) -> Result<()> {
    db.execute(
        "INSERT INTO agent_process (session, number, pid, pid_started, started_at, message) \
         VALUES (?1, (SELECT COALESCE(MAX(number), 0) + 1 FROM agent_process WHERE session = ?1), ?2, ?3, ?4, ?5)",
        params![
            session,
            process.pid,
            process.pid_started,
            process.started_at,
            process.message
        ],
    )
    .sql()?;
    Ok(())
}

/// Records that process <number> of <session> ended at <at> with <status>.
pub fn end_process(
    db: &Connection,
    session: &str,
    number: u32,
    at: i64,
    status: i32,
) -> Result<()> {
    let changed = db
        .execute(
            "UPDATE agent_process SET ended_at = ?3, exit_status = ?4 \
             WHERE session = ?1 AND number = ?2 AND ended_at IS NULL",
            params![session, number, at, status],
        )
        .sql()?;
    if changed != 1 {
        fail!("process {number} of agent {session} is not one that builds saw start and not end");
    }
    Ok(())
}

/// Records the context of <session>'s agent as <tokens> at <at>, with the
/// model's <window> when the stream has named it.
pub fn set_context(
    db: &Connection,
    session: &str,
    tokens: u64,
    window: Option<u64>,
    at: i64,
) -> Result<()> {
    db.execute(
        "UPDATE agent SET context_tokens = ?2, context_window = COALESCE(?3, context_window), context_read_at = ?4 \
         WHERE session = ?1",
        params![session, tokens as i64, window.map(|w| w as i64), at],
    )
    .sql()?;
    Ok(())
}

#[cfg(test)]
#[path = "agent_record_tests.rs"]
mod tests;

//! The agents `builds agent run` starts, in the project's store
//! (agent_record.sql). This is the only code that writes those tables.

use rusqlite::{Connection, OptionalExtension, params};

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
    /// The context of its last turn; None before the first turn.
    pub context: Option<Context>,
}

/// The context of an agent's last turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    pub tokens: u64,
    /// The model's window, known once the agent's result line has named it.
    pub window: Option<u64>,
    pub read_at: i64,
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
    add_process(db, &agent.session, process).map(drop)
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

/// Records <process> as the next process of <session>'s agent and returns its
/// number: the one after the last.
pub fn add_process(db: &Connection, session: &str, process: &Process) -> Result<u32> {
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
    db.query_row(
        "SELECT MAX(number) FROM agent_process WHERE session = ?1",
        [session],
        |row| row.get(0),
    )
    .sql()
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

fn row_agent(row: &rusqlite::Row<'_>) -> rusqlite::Result<Agent> {
    let role: String = row.get("role")?;
    let tier: String = row.get("tier")?;
    let worktree: Option<String> = row.get("worktree")?;
    let branch: Option<String> = row.get("branch")?;
    Ok(Agent {
        session: row.get("session")?,
        role: AgentRole::from_word(&role).expect("agent.role is checked"),
        model: row.get("model")?,
        tier: Tier::from_word(&tier).expect("agent.tier is checked"),
        folder: row.get("folder")?,
        brief: row.get("brief")?,
        places: worktree.zip(branch).into_iter().collect(),
        started_at: row.get("started_at")?,
        started_by: row.get("started_by")?,
        items: Vec::new(),
        context: {
            let tokens: Option<i64> = row.get("context_tokens")?;
            let window: Option<i64> = row.get("context_window")?;
            let read_at: Option<i64> = row.get("context_read_at")?;
            tokens.zip(read_at).map(|(tokens, read_at)| Context {
                tokens: tokens as u64,
                window: window.map(|window| window as u64),
                read_at,
            })
        },
    })
}

fn items_of(db: &Connection, session: &str) -> Result<Vec<u32>> {
    let mut statement = db
        .prepare("SELECT item FROM agent_item WHERE session = ?1 ORDER BY item")
        .sql()?;
    statement
        .query_map([session], |row| row.get(0))
        .sql()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .sql()
}

/// The agent of <session>, with its items, None when the store has none.
pub fn read(db: &Connection, session: &str) -> Result<Option<Agent>> {
    let found = db
        .query_row(
            "SELECT * FROM agent WHERE session = ?1",
            [session],
            row_agent,
        )
        .optional()
        .sql()?;
    let Some(mut agent) = found else {
        return Ok(None);
    };
    agent.items = items_of(db, session)?;
    Ok(Some(agent))
}

/// Every agent of the store, oldest first, with its items.
pub fn read_all(db: &Connection) -> Result<Vec<Agent>> {
    let mut statement = db
        .prepare("SELECT * FROM agent ORDER BY started_at, session")
        .sql()?;
    let mut agents = statement
        .query_map([], row_agent)
        .sql()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .sql()?;
    for agent in &mut agents {
        agent.items = items_of(db, &agent.session)?;
    }
    Ok(agents)
}

/// A process of an agent as the store keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRow {
    pub number: u32,
    pub process: Process,
    /// When builds saw it end and its exit status; None when builds saw no
    /// end.
    pub ended: Option<(i64, i32)>,
}

/// Every process of <session>'s agent, first first.
pub fn processes(db: &Connection, session: &str) -> Result<Vec<ProcessRow>> {
    let mut statement = db
        .prepare(
            "SELECT number, pid, pid_started, started_at, ended_at, exit_status, message \
             FROM agent_process WHERE session = ?1 ORDER BY number",
        )
        .sql()?;
    statement
        .query_map([session], |row| {
            let ended: Option<i64> = row.get(4)?;
            let status: Option<i32> = row.get(5)?;
            Ok(ProcessRow {
                number: row.get(0)?,
                process: Process {
                    pid: row.get(1)?,
                    pid_started: row.get(2)?,
                    started_at: row.get(3)?,
                    message: row.get(6)?,
                },
                ended: ended.zip(status),
            })
        })
        .sql()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .sql()
}

/// The sessions of <db>'s agents whose every item is closed.
pub fn sessions_with_closed_items(db: &Connection) -> Result<Vec<String>> {
    let mut statement = db
        .prepare(
            "SELECT a.session FROM agent a WHERE NOT EXISTS (\
                 SELECT 1 FROM agent_item i WHERE i.session = a.session \
                 AND NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = i.item))",
        )
        .sql()?;
    statement
        .query_map([], |row| row.get(0))
        .sql()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .sql()
}

#[cfg(test)]
#[path = "agent_record_tests.rs"]
mod tests;

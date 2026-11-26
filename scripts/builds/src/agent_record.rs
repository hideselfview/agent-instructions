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

/// A worktree where an agent's items run, with its branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub worktree: String,
    pub branch: String,
}

/// What an agent was started to do, with the data only that kind of agent
/// has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// Builds items to a brief, in the worktree and branch of each repo the
    /// items run in. The first is the folder the agent runs in.
    Implementing { places: Vec<Place> },
    /// Writes a plan or briefs, and commits nothing.
    Planner,
}

impl Work {
    pub fn role(&self) -> AgentRole {
        match self {
            Work::Implementing { .. } => AgentRole::Implementing,
            Work::Planner => AgentRole::Planner,
        }
    }

    /// The worktrees of an implementing agent; none for a planner.
    pub fn places(&self) -> &[Place] {
        match self {
            Work::Implementing { places } => places,
            Work::Planner => &[],
        }
    }
}

/// The context size at which an agent writes a handoff (`builds help
/// workflow`, HANDOFF, AUDITS AND FLAKY TESTS).
pub const HANDOFF_TOKENS: u64 = 550_000;

/// An agent as the store keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub session: String,
    pub work: Work,
    pub model: String,
    pub tier: Tier,
    pub folder: String,
    pub brief: String,
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
}

impl Context {
    /// Whether the agent is past the handoff size. A fresh agent continues
    /// the work of such an agent, so `agent resume` refuses it.
    pub fn past_handoff(self) -> bool {
        self.tokens >= HANDOFF_TOKENS
    }
}

/// A process of an agent, as builds records its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: i32,
    /// When the kernel says the process started; None when it had ended
    /// before the process list showed it.
    pub pid_started: Option<i64>,
    pub started_at: i64,
    /// The hash of the message builds sent the process.
    pub message: String,
}

/// Records a new agent, its places, its items and its first process, whole or
/// not at all.
pub fn start(db: &Connection, agent: &Agent, process: &Process) -> Result<()> {
    db.execute(
        "INSERT INTO agent (session, role, model, tier, folder, brief, started_at, started_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            agent.session,
            agent.work.role().word(),
            agent.model,
            agent.tier.word(),
            agent.folder,
            agent.brief,
            agent.started_at,
            agent.started_by,
        ],
    )
    .sql()?;
    add_places(db, &agent.session, agent.work.places())?;
    add_items(db, &agent.session, &agent.items, agent.started_at)?;
    add_process(db, &agent.session, process).map(drop)
}

fn add_places(db: &Connection, session: &str, places: &[Place]) -> Result<()> {
    for place in places {
        db.execute(
            "INSERT INTO agent_place (session, worktree, branch) VALUES (?1, ?2, ?3)",
            params![session, place.worktree, place.branch],
        )
        .sql()?;
    }
    Ok(())
}

/// Records that <items> joined <session>'s agent at <at>, whose work is now
/// <work>: its places replace the recorded ones. The caller's transaction
/// makes the writes one.
pub fn join(db: &Connection, session: &str, work: &Work, items: &[u32], at: i64) -> Result<()> {
    db.execute("DELETE FROM agent_place WHERE session = ?1", [session])
        .sql()?;
    add_places(db, session, work.places())?;
    add_items(db, session, items, at)
}

/// Records that <session>'s agent was given <items> at <at>. An item it
/// already had takes the later time.
fn add_items(db: &Connection, session: &str, items: &[u32], at: i64) -> Result<()> {
    for item in items {
        db.execute(
            "INSERT INTO agent_item (session, item, given_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (session, item) DO UPDATE SET given_at = excluded.given_at",
            params![session, item, at],
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

/// Records the context of <session>'s agent as <tokens>.
pub fn set_tokens(db: &Connection, session: &str, tokens: u64) -> Result<()> {
    db.execute(
        "UPDATE agent SET context_tokens = ?2 WHERE session = ?1",
        params![session, tokens as i64],
    )
    .sql()?;
    Ok(())
}

/// Records <window> as the context window of <session>'s agent, which has
/// at least one turn.
pub fn set_window(db: &Connection, session: &str, window: u64) -> Result<()> {
    db.execute(
        "UPDATE agent SET context_window = ?2 WHERE session = ?1",
        params![session, window as i64],
    )
    .sql()?;
    Ok(())
}

/// The columns of an `agent` row: the agent without its places and items.
struct Row {
    agent: Agent,
    implementing: bool,
}

fn row_agent(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let role: String = row.get("role")?;
    let tier: String = row.get("tier")?;
    let tokens: Option<i64> = row.get("context_tokens")?;
    let window: Option<i64> = row.get("context_window")?;
    Ok(Row {
        implementing: AgentRole::from_word(&role).expect("agent.role is checked")
            == AgentRole::Implementing,
        agent: Agent {
            session: row.get("session")?,
            work: Work::Planner,
            model: row.get("model")?,
            tier: Tier::from_word(&tier).expect("agent.tier is checked"),
            folder: row.get("folder")?,
            brief: row.get("brief")?,
            started_at: row.get("started_at")?,
            started_by: row.get("started_by")?,
            items: Vec::new(),
            context: tokens.map(|tokens| Context {
                tokens: tokens as u64,
                window: window.map(|window| window as u64),
            }),
        },
    })
}

/// <row>'s agent with its places and items.
fn complete(db: &Connection, row: Row) -> Result<Agent> {
    let mut agent = row.agent;
    if row.implementing {
        let mut statement = db
            .prepare(
                "SELECT worktree, branch FROM agent_place WHERE session = ?1 ORDER BY worktree",
            )
            .sql()?;
        let places = statement
            .query_map([&agent.session], |row| {
                Ok(Place {
                    worktree: row.get(0)?,
                    branch: row.get(1)?,
                })
            })
            .sql()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .sql()?;
        agent.work = Work::Implementing { places };
    }
    agent.items = items_of(db, &agent.session)?;
    Ok(agent)
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
    found.map(|row| complete(db, row)).transpose()
}

/// Every agent of the store, oldest first, with its items.
pub fn read_all(db: &Connection) -> Result<Vec<Agent>> {
    let mut statement = db
        .prepare("SELECT * FROM agent ORDER BY started_at, session")
        .sql()?;
    let rows = statement
        .query_map([], row_agent)
        .sql()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .sql()?;
    rows.into_iter().map(|row| complete(db, row)).collect()
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

/// The sessions of <db>'s agents whose folders no one needs: each is past the
/// handoff size, and each of its items is closed or taken over. A later agent
/// has taken an item over once an agent of the same role, given the item
/// after this one was, has ended a process with exit status 0 since then:
/// until that end, it may still have the handoff file to read.
pub fn sessions_done_with(db: &Connection) -> Result<Vec<String>> {
    let mut statement = db
        .prepare(
            "SELECT a.session FROM agent a WHERE a.context_tokens >= ?1 AND NOT EXISTS (\
                 SELECT 1 FROM agent_item i WHERE i.session = a.session \
                 AND NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = i.item) \
                 AND NOT EXISTS (\
                     SELECT 1 FROM agent_item j \
                     JOIN agent later ON later.session = j.session \
                     JOIN agent_process p ON p.session = j.session \
                     WHERE j.item = i.item AND j.given_at > i.given_at AND later.role = a.role \
                     AND p.exit_status = 0 AND p.ended_at >= j.given_at))",
        )
        .sql()?;
    statement
        .query_map([HANDOFF_TOKENS as i64], |row| row.get(0))
        .sql()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .sql()
}

/// Makes `agent_item` again as agent_record.sql writes it, with the time each
/// agent was given each item: a store from before kept none, and each item
/// takes the time its agent started.
pub fn add_given_at(db: &Connection) -> Result<()> {
    use crate::db::{remake_table, statement};
    let schema = include_str!("agent_record.sql");
    remake_table(
        db,
        "agent_item",
        statement(
            schema,
            "CREATE TABLE agent_item (",
            ") STRICT, WITHOUT ROWID;",
        ),
        |db| {
            db.execute_batch(
                "INSERT INTO agent_item (session, item, given_at) SELECT i.session, i.item, a.started_at FROM agent_item_old i JOIN agent a ON a.session = i.session",
            )
            .sql()
        },
        &[],
    )
}

#[cfg(test)]
#[path = "agent_record_tests.rs"]
mod tests;

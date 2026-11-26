//! `builds agent run`: starts an agent on items the coordinator took, or on
//! items a planner plans, and runs until the agent's process ends. builds
//! writes the agent's opening prompt from the items, the brief, the
//! project's hard limits and the workflow's sections, starts `claude` with
//! the flags of the role, records the session, and reads the agent's context
//! from the stream as it passes (`agent_stream`). The sibling commands
//! (`resume`, `list`, `show`) share this file's records and log.

use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::TransactionBehavior;

use crate::agent_launch::{self, Log, Session};
use crate::agent_prompt::{Brief, Item, Opening};
use crate::agent_record::{self, Agent, AgentRole, Place, Process, Work};
use crate::agent_stream::{Line, Report};
use crate::db::{self, Sql};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::args::{Args, current_dir};
use crate::lists::draft::find_brief;
use crate::lists::item::open_item;
use crate::lists::item_show::render_item;
use crate::lists::project_file;
use crate::lists::spec_check::Brief as BriefFile;
use crate::lists::store::{Store, resolve_by};
use crate::lists::{Id, Lists};
use crate::model_tier::Tier;
use crate::review::provider::Provider;
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "builds agent run --role implementing|planner N [M...] [--brief P] [--message-file F] [--project P] [--by WHO]\n       builds agent resume SESSION [--item N]... [--message-file F] [--project P]\n       builds agent list [--running] [--project P]\n       builds agent show SESSION [--project P]";

/// `builds agent <command>`; `agent start`, the hook, is `ack::agent`'s.
pub fn cmd_agent(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = current_dir()?;
    match args.split_first() {
        Some((command, rest)) if command == "run" => run(tool, rest, &cwd, &mut std::io::stdout()),
        Some((command, rest)) if command == "resume" => {
            resume(tool, rest, &cwd, &mut std::io::stdout())
        }
        Some((command, rest)) if command == "list" => {
            crate::agent_list::list(tool, rest, &cwd, &mut std::io::stdout())
        }
        Some((command, rest)) if command == "show" => {
            crate::agent_list::show(tool, rest, &cwd, &mut std::io::stdout())
        }
        Some((command, _)) => fail!("builds agent has no command {command}\nusage: {USAGE}"),
        None => fail!("usage: {USAGE}"),
    }
}

/// What `run` starts an agent on, from the items' records.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub work: Work,
    pub tier: Tier,
    pub ids: Vec<Id>,
    /// The brief's name: the items' brief, or the planner's `--brief`.
    pub brief: String,
}

/// Checks <ids> for an agent of <role> and says what it runs on. An
/// implementing agent takes items that run, all in the same worktrees with
/// the same brief and tier, and no `--brief`. A planner takes open items that
/// no agent runs, with `--brief`.
pub fn plan(lists: &Lists, role: AgentRole, ids: &[Id], brief: Option<&str>) -> Result<Plan> {
    if ids.is_empty() {
        fail!("usage: {USAGE}");
    }
    let items = ids
        .iter()
        .map(|id| open_item(lists, *id))
        .collect::<Result<Vec<_>>>()?;
    match role {
        AgentRole::Planner => {
            let Some(brief) = brief else {
                fail!("a planner needs the brief that says what to plan: pass --brief P");
            };
            for item in &items {
                if let Some(running) = item.running.first() {
                    fail!(
                        "{} runs in {}, and a planner starts only on items no agent runs",
                        item.id,
                        running.worktree
                    );
                }
            }
            Ok(Plan {
                work: Work::Planner,
                tier: Tier::Strongest,
                ids: ids.to_vec(),
                brief: brief.to_string(),
            })
        }
        AgentRole::Implementing => {
            if brief.is_some() {
                fail!(
                    "an implementing agent takes the brief its items run with, so --brief names none: it applies to a planner"
                );
            }
            let key = |item: &crate::lists::Item| {
                let mut places: Vec<(String, String)> = item
                    .running
                    .iter()
                    .map(|run| (run.worktree.clone(), run.branch.clone()))
                    .collect();
                places.sort();
                let run = item.running.first()?;
                Some((places, run.brief.clone(), run.tier))
            };
            let mut first = None;
            for item in &items {
                let Some(held) = key(item) else {
                    fail!(
                        "{} is not running: take it with `builds item take` before an agent starts on it",
                        item.id
                    );
                };
                match &first {
                    None => first = Some((item.id, held)),
                    Some((first_id, first_held)) if *first_held != held => fail!(
                        "{first_id} and {} run in different worktrees, with different briefs or at different tiers; start one agent for items that run together",
                        item.id
                    ),
                    Some(_) => {}
                }
            }
            let (_, (_, brief, tier)) = first.expect("the items are not empty");
            let places = items[0]
                .running
                .iter()
                .map(|run| Place {
                    worktree: run.worktree.clone(),
                    branch: run.branch.clone(),
                })
                .collect();
            Ok(Plan {
                work: Work::Implementing { places },
                tier,
                ids: ids.to_vec(),
                brief,
            })
        }
    }
}

/// A new Claude Code session id: a random version 4 UUID.
fn new_session_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&(crate::ack::text::random()? as u64).to_le_bytes());
    bytes[8..].copy_from_slice(&(crate::ack::text::random()? as u64).to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = crate::text::hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// The folder of the agents' worktrees for the repo whose main checkout
/// holds <brief>: where a planner runs.
fn planner_folder(tool: &Tool, repos: &[String], brief: &Path) -> Result<PathBuf> {
    for repo in repos {
        let main = tool.places.home.join(crate::host::clone_path(repo));
        if brief.starts_with(&main) {
            let folder = main.with_file_name(format!("{repo}-worktrees"));
            if !folder.is_dir() {
                fail!(
                    "the brief {} is in repo {repo}, whose worktrees folder {} does not exist",
                    brief.display(),
                    folder.display()
                );
            }
            return Ok(folder);
        }
    }
    fail!(
        "the brief {} is in the main checkout of none of the project's repos ({}), so builds finds no worktrees folder for the planner",
        brief.display(),
        repos.join(", ")
    )
}

/// What the folder of an agent holds on the build volume.
struct Folder {
    log: PathBuf,
    scratch: PathBuf,
}

fn make_folder(tool: &Tool, session: &str) -> Result<Folder> {
    tool.places.require_volume()?;
    let dir = tool.places.agents().join(session);
    let scratch = dir.join("scratch");
    std::fs::create_dir_all(&scratch).context(|| format!("creating {}", scratch.display()))?;
    Ok(Folder {
        log: dir.join(agent_launch::LOG_FILE),
        scratch,
    })
}

/// The name an agent shows in Claude Code's resume list.
fn agent_name(project: &str, role: AgentRole, items: &[u32]) -> String {
    let items: Vec<String> = items.iter().map(u32::to_string).collect();
    format!("{project} {} {}", items.join(" "), role.word())
}

/// Each of <ids> as `builds item show` prints it.
fn item_texts(lists: &Lists, ids: &[Id], now: i64) -> Result<Vec<String>> {
    ids.iter().map(|id| render_item(lists, *id, now)).collect()
}

/// <ids> with their <texts>, for the prompt and the resume message.
fn item_refs<'a>(ids: &[Id], texts: &'a [String]) -> Vec<Item<'a>> {
    ids.iter()
        .zip(texts)
        .map(|(id, text)| Item { id: id.0, text })
        .collect()
}

/// When the kernel says process <pid> started, None when the process list
/// does not show it, since it had ended already. Fails when builds cannot read
/// the process list.
fn kernel_start(pid: i32) -> Result<Option<i64>> {
    Ok(crate::procs::Processes::read()?
        .get(pid)
        .map(|process| process.started))
}

/// Records process <started> with <write>, in one transaction that also
/// holds what <write> records of the agent: the process starts at <now> and
/// is sent <message>. When recording fails, stops the process and fails.
fn record_process<T>(
    store: &Store,
    started: agent_launch::Started,
    message: &str,
    now: i64,
    write: impl FnOnce(&rusqlite::Connection, &Process) -> Result<T>,
) -> Result<(agent_launch::Started, T)> {
    let attempt = (|| -> Result<T> {
        let process = Process {
            pid: started.pid(),
            pid_started: kernel_start(started.pid())?,
            started_at: now,
            message: crate::review::texts::hash(message),
        };
        let mut conn = db::open(&store.database(), db::Make::Never)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .sql()?;
        let written = write(&tx, &process)?;
        tx.commit().sql()?;
        Ok(written)
    })();
    match attempt {
        Ok(written) => Ok((started, written)),
        Err(error) => {
            started.stop();
            Err(error)
        }
    }
}

/// `builds agent run`, printing to <out>.
fn run(tool: &Tool, args: &[String], cwd: &Path, out: &mut dyn Write) -> Result<()> {
    let args = Args::parse(args, &["role", "brief", "message-file"], &[])?;
    let word = args.require("role")?;
    let Some(role) = AgentRole::from_word(word) else {
        fail!("--role takes implementing or planner, not {word:?}");
    };
    let ids = args
        .positional()
        .iter()
        .map(|text| Id::parse(text))
        .collect::<Result<Vec<_>>>()?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let lists = store.load()?;
    let plan = plan(&lists, role, &ids, args.value("brief")?)?;
    let started_by = resolve_by(&args, cwd)?;
    let file = project_file::load(&tool.reviewer.rules_root, &lists.project)?;
    let repos = store.repos()?;
    let brief_path = find_brief(&tool.places, &repos, &plan.brief)?;
    let brief = BriefFile::read(&plan.brief, &brief_path)?;
    let message = match args.value("message-file")? {
        Some(path) => Some(
            std::fs::read_to_string(path).context(|| format!("reading the message file {path}"))?,
        ),
        None => None,
    };
    let texts = item_texts(&lists, &ids, tool.now)?;
    let items = item_refs(&ids, &texts);
    let folder = match &plan.work {
        Work::Implementing { places } => PathBuf::from(&places[0].worktree),
        Work::Planner => planner_folder(tool, &repos, &brief_path)?,
    };
    let session = new_session_id()?;
    let made = make_folder(tool, &session)?;
    let model = plan.tier.model(Provider::Claude);
    let prompt = Opening {
        project: &lists.project,
        description: &file.description,
        work: &plan.work,
        items: &items,
        brief: &Brief {
            name: &brief.name,
            text: &brief.text,
        },
        message: message.as_deref(),
        hard_limits_file: &file.path,
        hard_limits: &file.hard_limits,
        trailer: plan.tier.trailer(),
        scratch: &made.scratch.display().to_string(),
    }
    .text();
    let item_numbers: Vec<u32> = ids.iter().map(|id| id.0).collect();
    let name = agent_name(&lists.project, role, &item_numbers);
    let arguments = agent_launch::arguments(model, &name, Session::New(&session));
    let command = agent_launch::command(&tool.places.claude_program, &arguments, &folder);
    let started = agent_launch::start(command, &prompt, Log::append(&made.log)?)?;
    let agent = Agent {
        session: session.clone(),
        work: plan.work,
        model: model.to_string(),
        tier: plan.tier,
        folder: folder.display().to_string(),
        brief: plan.brief,
        started_at: tool.now,
        started_by,
        items: item_numbers,
        context: None,
    };
    let (started, ()) = record_process(&store, started, &prompt, tool.now, |db, process| {
        agent_record::start(db, &agent, process)
    })?;
    let _ = writeln!(
        out,
        "agent {session}: {} of project {}, model {model}, folder {}, log {}",
        role.word(),
        lists.project,
        folder.display(),
        made.log.display()
    );
    let _ = out.flush();
    drive(&store, &session, 1, model, started, out)
}

/// Reads the agent's stream until its process ends: checks what Claude Code
/// started against <model>, records the context of each turn, then records
/// the end of process <number>, prints the agent's last result whole and
/// the exit status, and fails with the status when it is not 0.
fn drive(
    store: &Store,
    session: &str,
    number: u32,
    model: &str,
    mut started: agent_launch::Started,
    out: &mut dyn Write,
) -> Result<()> {
    let mut report: Option<Report> = None;
    let mut context: Option<u64> = None;
    let mut context_failed = false;
    let read = started.read(|line| {
        match line {
            Line::Init(init) => agent_launch::check_init(model, &init)?,
            Line::Turn { context: tokens } => {
                context = Some(tokens);
                let recorded = db::open(&store.database(), db::Make::Never)
                    .and_then(|conn| agent_record::set_tokens(&conn, session, tokens));
                if let Err(error) = recorded
                    && !context_failed
                {
                    context_failed = true;
                    note(format!(
                        "builds could not record the agent's context, which the log still holds: {}",
                        error.text()
                    ));
                }
            }
            Line::Result(result) => report = Some(result),
            Line::Other => {}
        }
        Ok(())
    });
    let status = started.finish()?;
    let ended = (|| -> Result<()> {
        let conn = db::open(&store.database(), db::Make::Never)?;
        if let (Some(_), Some(window)) = (context, report.as_ref().and_then(|r| r.window)) {
            agent_record::set_window(&conn, session, window)?;
        }
        agent_record::end_process(&conn, session, number, crate::app_timing::now(), status)
    })();
    read?;
    ended?;
    match &report {
        Some(report) => {
            let _ = writeln!(out, "{}", report.text);
        }
        None => {
            let _ = writeln!(out, "the agent printed no result");
        }
    }
    let _ = writeln!(out, "agent {session} ended with exit status {status}");
    if status != 0 {
        return Err(Fail::Exit(status));
    }
    Ok(())
}

const RESUME_USAGE: &str =
    "builds agent resume SESSION [--item N]... [--message-file F] [--project P]";

/// Fails unless each of <ids> may join the agent <agent>: an implementing
/// agent takes items that run in its worktree, a planner open items no agent
/// runs.
fn require_joinable(lists: &Lists, agent: &Agent, ids: &[Id]) -> Result<()> {
    for id in ids {
        let item = open_item(lists, *id)?;
        match &agent.work {
            Work::Implementing { places } => {
                let joins = item.running.iter().any(|run| {
                    places
                        .iter()
                        .any(|place| place.worktree == run.worktree && place.branch == run.branch)
                });
                if !joins {
                    fail!(
                        "{id} does not run in the worktree of agent {}: take it there with `builds item take` first",
                        agent.session
                    );
                }
            }
            Work::Planner => {
                if let Some(run) = item.running.first() {
                    fail!(
                        "{id} runs in {}, and a planner takes only items no agent runs",
                        run.worktree
                    );
                }
            }
        }
    }
    Ok(())
}

/// `builds agent resume`, printing to <out>.
fn resume(tool: &Tool, args: &[String], cwd: &Path, out: &mut dyn Write) -> Result<()> {
    let args = Args::parse(args, &["item", "message-file"], &[])?;
    let [session] = args.exactly(1, RESUME_USAGE)? else {
        unreachable!("exactly one")
    };
    let ids = args
        .values("item")
        .into_iter()
        .map(Id::parse)
        .collect::<Result<Vec<_>>>()?;
    let message_file = args.value("message-file")?;
    if ids.is_empty() && message_file.is_none() {
        fail!(
            "give the agent something to read: --item N or --message-file F\nusage: {RESUME_USAGE}"
        );
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let lists = store.load()?;
    let agent = {
        let conn = db::open(&store.database(), db::Make::Never)?;
        agent_record::read(&conn, session)?
    };
    let Some(agent) = agent else {
        fail!("project {} records no agent {session}", lists.project);
    };
    require_joinable(&lists, &agent, &ids)?;
    let base = match message_file {
        Some(path) => {
            std::fs::read_to_string(path).context(|| format!("reading the message file {path}"))?
        }
        None => String::new(),
    };
    let texts = item_texts(&lists, &ids, tool.now)?;
    let new_items = item_refs(&ids, &texts);
    let mut all_ids: Vec<u32> = agent.items.clone();
    all_ids.extend(ids.iter().map(|id| id.0));
    all_ids.sort_unstable();
    all_ids.dedup();
    let message = crate::agent_prompt::resume_message(
        &lists.project,
        &base,
        agent.work.places(),
        &all_ids,
        &new_items,
        agent.tier.trailer(),
    );
    let new_ids: Vec<u32> = ids.iter().map(|id| id.0).collect();
    if let Some(socket) = running_socket(tool, session)? {
        let inbox = crate::inbox::Messaging::new(&socket, None)?;
        if tool.places.sessions().send(&inbox, &message)? == crate::inbox::Sent::Delivered {
            let conn = db::open(&store.database(), db::Make::Never)?;
            agent_record::add_items(&conn, session, &new_ids)?;
            let _ = writeln!(out, "agent {session}: the message reached its inbox");
            return Ok(());
        }
    }
    let log = tool
        .places
        .agents()
        .join(session)
        .join(agent_launch::LOG_FILE);
    let name = agent_name(&lists.project, agent.work.role(), &agent.items);
    let arguments = agent_launch::arguments(&agent.model, &name, Session::Resume(session));
    let command = agent_launch::command(
        &tool.places.claude_program,
        &arguments,
        Path::new(&agent.folder),
    );
    let started = agent_launch::start(command, &message, Log::append(&log)?)?;
    let (started, number) = record_process(&store, started, &message, tool.now, |db, process| {
        agent_record::add_items(db, session, &new_ids)?;
        agent_record::add_process(db, session, process)
    })?;
    let _ = writeln!(
        out,
        "agent {session}: resumed in {}, log {}",
        agent.folder,
        log.display()
    );
    let _ = out.flush();
    drive(&store, session, number, &agent.model, started, out)
}

/// The inbox socket of the running Claude Code session <session>, None when
/// no record names it as running or the record names no socket.
fn running_socket(tool: &Tool, session: &str) -> Result<Option<String>> {
    let running = crate::claude_sessions::running_now(&tool.places.claude_sessions())
        .map_err(|why| Fail::Message(format!("reading the running Claude Code sessions: {why}")))?;
    Ok(running
        .into_iter()
        .find(|running| running.session_id == session)
        .and_then(|running| running.messaging_socket))
}

#[cfg(test)]
#[path = "agent_command_tests.rs"]
mod tests;

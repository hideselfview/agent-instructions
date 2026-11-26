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
use crate::claude_sessions::Running;
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

/// The worktree and branch of each repo <item> runs in, by worktree; none
/// when it does not run.
fn run_places(item: &crate::lists::Item) -> Vec<Place> {
    let mut places: Vec<Place> = item
        .running
        .iter()
        .map(|run| Place {
            worktree: run.worktree.clone(),
            branch: run.branch.clone(),
        })
        .collect();
    places.sort_by(|a, b| a.worktree.cmp(&b.worktree));
    places
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
                let run = item.running.first()?;
                Some((run_places(item), run.brief.clone(), run.tier))
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
            let (_, (places, brief, tier)) = first.expect("the items are not empty");
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

/// An agent as a resume with new items leaves it.
struct Joined {
    work: Work,
    /// The items that run in the agent's worktrees, which each land command
    /// names; none for a planner.
    landing: Vec<u32>,
}

/// What <agent> works on once <ids> join it, while the sessions <running>
/// run. A planner takes open items no agent runs. An implementing agent takes
/// items that run, in any worktrees and on any branch, unless another of
/// <agents> that runs now has one there. Its places become the ones those
/// items run in, and the earlier items it still has there must run in the
/// same ones.
fn join(
    lists: &Lists,
    agent: &Agent,
    ids: &[Id],
    agents: &[Agent],
    running: &[Running],
) -> Result<Joined> {
    let items = ids
        .iter()
        .map(|id| open_item(lists, *id))
        .collect::<Result<Vec<_>>>()?;
    let unchanged = Joined {
        work: agent.work.clone(),
        landing: Vec::new(),
    };
    if agent.work == Work::Planner {
        for item in &items {
            if let Some(run) = item.running.first() {
                fail!(
                    "{} runs in {}, and a planner takes only items no agent runs",
                    item.id,
                    run.worktree
                );
            }
        }
        return Ok(unchanged);
    }
    for item in &items {
        if item.running.is_empty() {
            fail!(
                "{} is not running: take it with `builds item take` before an agent resumes on it",
                item.id
            );
        }
        let others = agents.iter().filter(|other| {
            other.session != agent.session
                && other.items.contains(&item.id.0)
                && running_session(running, &other.session).is_some()
        });
        for other in others {
            let theirs = item.running.iter().find(|run| {
                let places = other.work.places();
                places.iter().any(|place| place.worktree == run.worktree)
            });
            if let Some(run) = theirs {
                fail!(
                    "agent {} runs now and has {} in {}, so builds does not give {} to agent {}",
                    other.session,
                    item.id,
                    run.worktree,
                    item.id,
                    agent.session
                );
            }
        }
    }
    let Some(first) = items.first() else {
        return Ok(unchanged);
    };
    let places = run_places(first);
    let earlier = agent
        .items
        .iter()
        .filter_map(|number| lists.item(Id(*number)))
        .filter(|item| {
            item.running
                .iter()
                .any(|run| places.iter().any(|place| place.worktree == run.worktree))
        });
    let mut landing = Vec::new();
    for item in items.iter().copied().chain(earlier) {
        if run_places(item) != places {
            fail!(
                "{} and {} run in different worktrees; resume an agent on items that run together",
                first.id,
                item.id
            );
        }
        landing.push(item.id.0);
    }
    landing.sort_unstable();
    landing.dedup();
    Ok(Joined {
        work: Work::Implementing { places },
        landing,
    })
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
    let agents = {
        let conn = db::open(&store.database(), db::Make::Never)?;
        agent_record::read_all(&conn)?
    };
    let Some(agent) = agents.iter().find(|agent| agent.session == *session) else {
        fail!("project {} records no agent {session}", lists.project);
    };
    if let Some(context) = agent.context.filter(|context| context.past_handoff()) {
        fail!(
            "agent {session} has {} of context, past the handoff size of {}, so builds does not resume it: start a fresh agent on its items with `builds agent run`. For an item that still runs, first give it a brief that names the agent's handoff file with `builds item handoff N --brief P`; the agent keeps its scratch files in {}",
            crate::agent_list::thousands(context.tokens),
            crate::agent_list::thousands(agent_record::HANDOFF_TOKENS),
            tool.places.agents().join(session).join("scratch").display()
        );
    }
    let running = crate::claude_sessions::running_now(&tool.places.claude_sessions())
        .map_err(|why| Fail::Message(format!("reading the running Claude Code sessions: {why}")))?;
    let joined = join(&lists, agent, &ids, &agents, &running)?;
    let base = match message_file {
        Some(path) => {
            std::fs::read_to_string(path).context(|| format!("reading the message file {path}"))?
        }
        None => String::new(),
    };
    let texts = item_texts(&lists, &ids, tool.now)?;
    let new_items = item_refs(&ids, &texts);
    let message = crate::agent_prompt::resume_message(
        &lists.project,
        &base,
        joined.work.places(),
        &joined.landing,
        &new_items,
        agent.tier.trailer(),
    );
    let new_ids: Vec<u32> = ids.iter().map(|id| id.0).collect();
    let socket = running_session(&running, session).and_then(|own| own.messaging_socket.as_deref());
    if let Some(socket) = socket {
        let inbox = crate::inbox::Messaging::new(socket, None)?;
        if tool.places.sessions().send(&inbox, &message)? == crate::inbox::Sent::Delivered {
            let mut conn = db::open(&store.database(), db::Make::Never)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .sql()?;
            agent_record::join(&tx, session, &joined.work, &new_ids, tool.now)?;
            tx.commit().sql()?;
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
    // Claude Code finds a session by its id from any folder, so the new
    // process runs where the agent's items now run.
    let folder = match joined.work.places().first() {
        Some(place) => &place.worktree,
        None => &agent.folder,
    };
    let command = agent_launch::command(&tool.places.claude_program, &arguments, Path::new(folder));
    let started = agent_launch::start(command, &message, Log::append(&log)?)?;
    let (started, number) = record_process(&store, started, &message, tool.now, |db, process| {
        agent_record::join(db, session, &joined.work, &new_ids, tool.now)?;
        agent_record::set_folder(db, session, folder)?;
        agent_record::add_process(db, session, process)
    })?;
    let _ = writeln!(
        out,
        "agent {session}: resumed in {folder}, log {}",
        log.display()
    );
    let _ = out.flush();
    drive(&store, session, number, &agent.model, started, out)
}

/// The record in <running> of Claude Code session <session>, None when it
/// does not run.
fn running_session<'a>(running: &'a [Running], session: &str) -> Option<&'a Running> {
    running.iter().find(|running| running.session_id == session)
}

#[cfg(test)]
#[path = "agent_command_tests.rs"]
mod tests;

//! The agents builds knows, each by the id Claude Code gives it.
//!
//! Claude Code gives every subagent an id, which a hook's input names
//! (`agent_id`) but which no shell of the agent sees. The main agent of a
//! session has no such id, and builds names it by its session id. When
//! Claude Code starts or resumes a session, or starts or resumes a subagent,
//! a hook runs `builds agent start`, which registers the agent's id and tells
//! the agent its id. The agent then passes `--agent ID` to every builds
//! command, or sets `BUILDS_AGENT` for a script that runs builds.
//!
//! builds keeps one file per registered agent, at
//! `<lists' parent>/agents/<session>/<agent>`, beside the projects' stores,
//! so that the gate of every project reads the same agents. The file is empty,
//! except after Claude Code compacts the agent, until the agent acknowledges
//! the text again (`mark_compacted`).

use std::path::PathBuf;

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;

/// The variable that names the agent that runs a builds command. `--agent
/// ID` sets it for the command (`take_flag`).
pub const VARIABLE: &str = "BUILDS_AGENT";

/// The folder of the registered agents, one folder per session, beside the
/// lists folder. Fails when the lists folder has no parent.
pub fn dir(places: &Places) -> Result<PathBuf> {
    let Some(parent) = places.lists.parent() else {
        fail!(
            "builds keeps the agents it registers beside the lists folder, and {} has no parent folder",
            places.lists.display()
        );
    };
    Ok(parent.join("agents"))
}

/// The session folders of the agents folder (`dir`) that belong to no session
/// among the <live> ones: the registrations nothing reads, since only an
/// agent of a running session runs a command that reads one. <now> is
/// seconds since 1970.
pub fn ended_sessions(
    places: &Places,
    live: &std::collections::HashSet<std::ffi::OsString>,
    now: i64,
) -> std::result::Result<Vec<PathBuf>, String> {
    let agents = dir(places).map_err(|error| error.text())?;
    if !agents.is_dir() {
        return Ok(Vec::new());
    }
    crate::prune::old_entries(&[agents], 0, places.uid, now, |name, meta| {
        meta.is_dir() && !name.starts_with('.') && !live.contains(std::ffi::OsStr::new(name))
    })
}

/// Whether <c> may appear in a session or agent id.
pub fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// The file that registers agent <agent> of <session>. Fails for an id
/// that holds anything but letters, digits, `-` and `_`, since the ids name
/// folders and files.
fn file(places: &Places, session: &str, agent: &str) -> Result<PathBuf> {
    for (what, id) in [("session id", session), ("agent id", agent)] {
        if id.is_empty() || !id.chars().all(is_id_char) {
            fail!("{id:?} is no {what}: an id holds only letters, digits, - and _");
        }
    }
    Ok(dir(places)?.join(session).join(agent))
}

/// Registers agent <agent> of <session>. Registering it again changes
/// nothing.
pub fn register(places: &Places, session: &str, agent: &str) -> Result<()> {
    let path = file(places, session, agent)?;
    let folder = path.parent().expect("an agent's file has a folder");
    std::fs::create_dir_all(folder).context(|| format!("creating {}", folder.display()))?;
    std::fs::write(&path, "").context(|| format!("writing {}", path.display()))
}

/// What the file of an agent that Claude Code compacted holds, until the agent
/// acknowledges the text again.
const COMPACTED: &str = "compacted\n";

/// Records that Claude Code compacted agent <agent> of <session>, which the
/// agent's next reading tells it. Fails for an agent builds did not register.
pub fn mark_compacted(places: &Places, session: &str, agent: &str) -> Result<()> {
    let path = file(places, session, agent)?;
    std::fs::write(&path, COMPACTED).context(|| format!("writing {}", path.display()))
}

/// Whether Claude Code compacted agent <agent> of <session> since it last
/// acknowledged the text.
pub fn was_compacted(places: &Places, session: &str, agent: &str) -> Result<bool> {
    let path = file(places, session, agent)?;
    let held = std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    Ok(held == COMPACTED)
}

/// Records that agent <agent> of <session> acknowledged the text since Claude
/// Code compacted it.
pub fn clear_compacted(places: &Places, session: &str, agent: &str) -> Result<()> {
    let path = file(places, session, agent)?;
    std::fs::write(&path, "").context(|| format!("writing {}", path.display()))
}

/// Whether builds registered agent <agent> of <session>.
pub fn is_registered(places: &Places, session: &str, agent: &str) -> Result<bool> {
    let path = file(places, session, agent)?;
    match std::fs::metadata(&path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}

/// <args> without `--agent ID`, with `BUILDS_AGENT` set to ID for this
/// command and every command it runs. Fails as `split_flag` and `chosen` do.
pub fn take_flag(args: &[String]) -> Result<Vec<String>> {
    let (rest, given) = split_flag(args)?;
    if let Some(id) = chosen(given.as_deref(), from_env()?.as_deref())? {
        // SAFETY: no other thread runs yet, so none reads the environment
        // while set_var writes it.
        unsafe { std::env::set_var(VARIABLE, id) };
    }
    Ok(rest)
}

/// <args> without `--agent ID`, which may come anywhere before a `--`, and
/// the ID. Fails when `--agent` has no value or comes twice.
fn split_flag(args: &[String]) -> Result<(Vec<String>, Option<String>)> {
    let mut rest = Vec::with_capacity(args.len());
    let mut given: Option<String> = None;
    let mut items = args.iter();
    while let Some(arg) = items.next() {
        if arg == "--" {
            rest.push(arg.clone());
            rest.extend(items.by_ref().cloned());
            break;
        }
        if arg != "--agent" {
            rest.push(arg.clone());
            continue;
        }
        let Some(id) = items.next() else {
            fail!("--agent needs a value: your builds agent id");
        };
        if given.is_some() {
            fail!("--agent comes twice; pass it once");
        }
        given = Some(id.clone());
    }
    Ok((rest, given))
}

/// The agent that runs a command given `--agent` <flag> and `BUILDS_AGENT`
/// <env>: either one, and refused when they name different agents.
fn chosen(flag: Option<&str>, env: Option<&str>) -> Result<Option<String>> {
    match (flag, env) {
        (Some(flag), Some(env)) if flag != env => {
            fail!("--agent names {flag}, and {VARIABLE} names {env}; pass one agent id")
        }
        (flag, env) => Ok(flag.or(env).map(str::to_string)),
    }
}

/// The agent that runs this command, from `BUILDS_AGENT`, or None when the
/// variable is not set. Fails when it is set but empty or not Unicode.
pub fn from_env() -> Result<Option<String>> {
    match std::env::var(VARIABLE) {
        Ok(id) if id.is_empty() => {
            fail!("{VARIABLE} holds an empty value; set it to your builds agent id, or unset it")
        }
        Ok(id) => Ok(Some(id)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            fail!("{VARIABLE} holds a value that is not Unicode; set it to your builds agent id")
        }
    }
}

/// The agent that runs a builds command, as builds names it: the Claude Code
/// session it runs in and its agent id. A session's main agent has the
/// session's id as its agent id; every agent the session dispatches has an id
/// of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub session: String,
    pub id: String,
}

impl Caller {
    /// The agent that runs this command, from `CLAUDE_CODE_SESSION_ID` and
    /// `BUILDS_AGENT`; None for the person's own terminal, which has no
    /// session. Fails as `of` does.
    pub fn from_env() -> Result<Option<Caller>> {
        let session = std::env::var(super::SESSION).ok();
        Caller::of(session.as_deref(), from_env()?.as_deref())
    }

    /// The agent of <session> with id <agent>: None with no session, and a
    /// failure for a session with no agent id, since builds cannot tell
    /// that session's agents apart.
    pub fn of(session: Option<&str>, agent: Option<&str>) -> Result<Option<Caller>> {
        match (session.filter(|s| !s.is_empty()), agent) {
            (None, _) => Ok(None),
            (Some(_), None) => fail!(
                "builds names each agent by the id Claude Code gives it, and this command names none: pass --agent ID, or set {VARIABLE}=ID"
            ),
            (Some(session), Some(id)) => Ok(Some(Caller {
                session: session.to_string(),
                id: id.to_string(),
            })),
        }
    }

    /// Whether a session dispatched this agent: it is not the session's main
    /// agent. A dispatched agent shares its session's inbox.
    pub fn is_dispatched(&self) -> bool {
        self.id != self.session
    }
}

/// What Claude Code's hooks pass on standard input, as builds reads it.
#[derive(serde::Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub hook_event_name: String,
    /// The subagent that triggered the hook; absent for the main agent.
    pub agent_id: Option<String>,
    /// How a session started, for a SessionStart hook.
    pub source: Option<String>,
}

impl HookInput {
    /// Reads and parses the input Claude Code passes a hook on standard
    /// input.
    pub fn from_stdin() -> Result<HookInput> {
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
            .context(|| "reading the hook's input".to_string())?;
        HookInput::parse(&input)
    }

    /// Parses a hook's standard input <input>.
    pub fn parse(input: &str) -> Result<HookInput> {
        serde_json::from_str(input).map_err(|error| {
            fail::Fail::Message(format!(
                "the input is not the input of a Claude Code hook: {error}"
            ))
        })
    }

    /// The agent the hook ran for: the subagent's id, else the session id,
    /// which names the main agent.
    pub fn agent(&self) -> &str {
        self.agent_id.as_deref().unwrap_or(&self.session_id)
    }
}

/// The sentence that tells agent <agent> its id and how to pass it.
pub fn told(agent: &str) -> String {
    format!(
        "Your builds agent id is {agent}. Pass --agent {agent} to every builds command, or set {VARIABLE}={agent} in front of a script that runs builds, such as `{VARIABLE}={agent} scripts/cargo.sh test`."
    )
}

/// The JSON a hook of event <event> prints to add <context> to the agent's
/// context.
pub fn hook_output(event: &str, context: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": context,
        }
    })
    .to_string()
}

/// `builds agent start`: what the SessionStart hook (for a session that
/// starts, resumes, clears or forks) and the SubagentStart hook run. Reads the
/// hook's input from standard input, registers the agent and prints the
/// hook's output, which tells the agent its id.
pub fn cmd_agent(places: &Places, args: &[String]) -> Result<()> {
    if args != ["start"] {
        fail!(
            "usage: builds agent start   (a Claude Code hook runs it, with the hook's input on standard input)"
        );
    }
    println!("{}", start(places, HookInput::from_stdin()?)?);
    Ok(())
}

/// Registers the agent the hook input <hook> names, and returns the hook's
/// output.
pub(super) fn start(places: &Places, hook: HookInput) -> Result<String> {
    match (hook.hook_event_name.as_str(), hook.source.as_deref()) {
        ("SubagentStart", _) if hook.agent_id.is_none() => {
            fail!("the SubagentStart hook's input names no agent_id")
        }
        ("SubagentStart", _) => {}
        ("SessionStart", Some("compact")) => fail!(
            "after a compaction the SessionStart hook runs `builds ack --compacted`, not `builds agent start`"
        ),
        ("SessionStart", _) => {}
        (other, _) => fail!(
            "`builds agent start` runs from the SessionStart and SubagentStart hooks, not from {other}"
        ),
    }
    let agent = hook.agent();
    register(places, &hook.session_id, agent)?;
    Ok(hook_output(&hook.hook_event_name, &told(agent)))
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;

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
//! builds keeps one empty file per registered agent, at
//! `<lists' parent>/agents/<session>/<agent>`, beside the projects' stores,
//! so that the gate of every project reads the same agents.

use std::path::PathBuf;

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;

/// The variable that names the agent that runs a builds command. `--agent
/// ID` sets it for the command (`take_flag`).
pub const VARIABLE: &str = "BUILDS_AGENT";

/// The folder of the registered agents, one folder per session.
pub fn dir(places: &Places) -> PathBuf {
    places
        .lists
        .parent()
        .unwrap_or(&places.lists)
        .join("agents")
}

/// The file that registers agent <agent> of <session>. Fails for an id
/// that holds anything but letters, digits, `-` and `_`, since the ids name
/// folders and files.
fn file(places: &Places, session: &str, agent: &str) -> Result<PathBuf> {
    for (what, id) in [("session id", session), ("agent id", agent)] {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            fail!("{id:?} is no {what}: an id holds only letters, digits, - and _");
        }
    }
    Ok(dir(places).join(session).join(agent))
}

/// Registers agent <agent> of <session>. Registering it again changes
/// nothing.
pub fn register(places: &Places, session: &str, agent: &str) -> Result<()> {
    let path = file(places, session, agent)?;
    let folder = path.parent().expect("an agent's file has a folder");
    std::fs::create_dir_all(folder).context(|| format!("creating {}", folder.display()))?;
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
    if let Some(id) = chosen(given.as_deref(), from_env().as_deref())? {
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

/// The agent that runs this command, from `BUILDS_AGENT`, or None.
pub fn from_env() -> Option<String> {
    std::env::var(VARIABLE).ok().filter(|id| !id.is_empty())
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
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .context(|| "reading the hook's input".to_string())?;
    println!("{}", start(places, &input)?);
    Ok(())
}

/// Registers the agent the hook input <input> names, and returns the hook's
/// output.
pub(super) fn start(places: &Places, input: &str) -> Result<String> {
    let hook = HookInput::parse(input)?;
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

//! The model each role takes, and health's finding for an implementing agent
//! that runs another one. builds help workflow (PLAN, BRIEF, EXECUTE) states
//! the same tiers in words: the coordinator names the implementation tier's
//! model on each dispatch, and nothing else notices when it names another.
//!
//! An implementing agent is an agent process whose working folder is a side
//! worktree where an item runs. Its model is the value of its command line's
//! model flag.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::health::Advice;
use crate::lists::Id;
use crate::procs::Processes;

/// The agent program an agent process runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    Claude,
    Codex,
}

/// The model each client runs for one role.
pub struct Tier {
    pub claude: &'static str,
    pub codex: &'static str,
}

impl Tier {
    fn of(&self, client: Client) -> &'static str {
        match client {
            Client::Claude => self.claude,
            Client::Codex => self.codex,
        }
    }
}

/// Implementing: the role of an agent that works an item in a side worktree.
pub const IMPLEMENTATION: Tier = Tier {
    claude: "claude-sonnet-5-5",
    codex: "gpt-6.2-sol",
};

/// A running agent process.
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub pid: i32,
    pub client: Client,
    pub cwd: PathBuf,
    pub argv: Vec<String>,
}

/// A claimed side worktree and the open items that run in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Worktree {
    pub top: PathBuf,
    pub label: String,
    pub items: Vec<Id>,
}

/// The model <argv> names with `--model`, `--model=`, `-m` or `-m=`.
pub fn model_of(argv: &[String]) -> Option<&str> {
    let mut words = argv.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if matches!(word, "--model" | "-m") {
            return words.next();
        }
        if let Some(value) = word
            .strip_prefix("--model=")
            .or_else(|| word.strip_prefix("-m="))
        {
            return Some(value);
        }
    }
    None
}

/// One line for each agent among <agents> that works in a worktree of
/// <worktrees> where an item runs and does not run the implementation tier's
/// model for its client.
pub fn findings(agents: &[Agent], worktrees: &[Worktree]) -> Vec<Advice> {
    let mut lines = Vec::new();
    for agent in agents {
        let Some(worktree) = worktrees
            .iter()
            .find(|w| !w.items.is_empty() && agent.cwd.starts_with(&w.top))
        else {
            continue;
        };
        let wanted = IMPLEMENTATION.of(agent.client);
        let runs = match model_of(&agent.argv) {
            Some(model) if model == wanted => continue,
            Some(model) => format!("runs {model}"),
            None => "names no model".to_string(),
        };
        let (noun, items) = match worktree.items.as_slice() {
            [one] => ("item", one.0.to_string()),
            many => (
                "items",
                many.iter()
                    .map(|id| id.0.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        };
        lines.push(Advice::said(format!(
            "model: the agent (pid {}) in {} for {noun} {items} {runs}, and an implementing agent takes {wanted}. Stop the agent at its next clean point and continue the {noun} with an agent that runs {wanted}",
            agent.pid, worktree.label,
        )));
    }
    lines
}

/// The agent processes among <processes>: each process a running Claude Code
/// session (<claude>, the pids of `claude_sessions`) or a `codex` process,
/// with its working folder. What builds cannot read of one is a line in the
/// second value.
pub fn read(processes: &Processes, claude: &HashSet<i32>) -> (Vec<Agent>, Vec<String>) {
    let mut agents = Vec::new();
    let mut unread = Vec::new();
    for process in &processes.list {
        let client = if claude.contains(&process.pid) {
            Client::Claude
        } else if process.name == "codex" {
            Client::Codex
        } else {
            continue;
        };
        let argv = match crate::procs::live_args(process.pid) {
            Ok(Some(argv)) => argv,
            Ok(None) => continue,
            Err(error) => {
                unread.push(format!(
                    "{error}, so health cannot tell which model the agent runs"
                ));
                continue;
            }
        };
        if let Some(cwd) = crate::procs::cwd(process.pid) {
            agents.push(Agent {
                pid: process.pid,
                client,
                cwd,
                argv,
            });
        }
    }
    (agents, unread)
}

#[cfg(test)]
#[path = "model_tier_tests.rs"]
mod tests;

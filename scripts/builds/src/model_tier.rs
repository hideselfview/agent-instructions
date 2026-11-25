//! The model an implementing agent takes, and health's finding for one that
//! runs another. builds help workflow (PLAN, BRIEF, EXECUTE) states the same
//! tiers in words: the coordinator names the implementation tier's model on
//! each dispatch, and nothing else notices when it names another.
//!
//! An implementing agent is an agent process whose working folder is a side
//! worktree where an item runs. Its model is the value of its command line's
//! model flag.

use std::collections::HashSet;

use crate::health::Advice;
use crate::lists::Lists;
use crate::procs::{Place, Processes};
use crate::review::provider::Provider;

/// The model an implementing agent of <provider> runs.
pub fn implementation_model(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude-sonnet-5-5",
        Provider::Codex => "gpt-6.2-sol",
    }
}

/// A running agent process and the place its working folder is in.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Agent {
    pub pid: i32,
    pub provider: Provider,
    /// Its index among the places the machine was sampled with.
    pub place: usize,
    pub argv: Vec<String>,
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

/// One line for each of <agents> that works in a place where an item of
/// <lists> runs and does not run the implementation model of its provider.
pub fn findings(agents: &[Agent], places: &[Place], lists: &[&Lists]) -> Vec<Advice> {
    let mut lines = Vec::new();
    for agent in agents {
        let place = &places[agent.place];
        let worktree = place.top.display().to_string();
        let items: Vec<u32> = lists
            .iter()
            .flat_map(|lists| crate::lists::running::running_in(lists, &worktree))
            .map(|item| item.id.0)
            .collect();
        if items.is_empty() {
            continue;
        }
        let wanted = implementation_model(agent.provider);
        let runs = match model_of(&agent.argv) {
            Some(model) if model == wanted => continue,
            Some(model) => format!("runs {model}"),
            None => "names no model".to_string(),
        };
        let (noun, ids) = match items.as_slice() {
            [one] => ("item", one.to_string()),
            many => (
                "items",
                many.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        };
        lines.push(Advice::said(format!(
            "model: the agent (pid {}) in {} for {noun} {ids} {runs}, and an implementing agent takes {wanted}. Stop the agent at its next clean point and continue the {noun} with an agent that runs {wanted}",
            agent.pid, place.label,
        )));
    }
    lines
}

/// The agent processes among <processes> that work in one of <places>: each
/// Claude Code session (a process whose pid is in <claude>, the pids of
/// `claude_sessions`) and each `codex` process, except those `builds`
/// started. The second value has a line for each agent process whose
/// arguments or working folder builds cannot read.
pub fn read(
    processes: &Processes,
    claude: &HashSet<i32>,
    places: &[Place],
) -> (Vec<Agent>, Vec<String>) {
    let mut agents = Vec::new();
    let mut unread = Vec::new();
    for process in &processes.list {
        // builds starts model calls of its own, such as the reviewers
        // (review/provider.rs), which run the strongest tier in the worktree
        // they read: they are no implementing agent.
        if crate::queue::is_builds(processes, process.ppid) {
            continue;
        }
        let provider = if claude.contains(&process.pid) {
            Provider::Claude
        } else if process.name == "codex" {
            Provider::Codex
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
        let Some(cwd) = crate::procs::cwd(process.pid) else {
            if crate::procs::runs(process.pid) {
                unread.push(format!(
                    "builds cannot read the working folder of agent process {}, which still runs, so health cannot tell which worktree it works in",
                    process.pid
                ));
            }
            continue;
        };
        if let Some(place) = crate::procs::place_of(places, &cwd) {
            agents.push(Agent {
                pid: process.pid,
                provider,
                place,
                argv,
            });
        }
    }
    (agents, unread)
}

#[cfg(test)]
#[path = "model_tier_tests.rs"]
mod tests;

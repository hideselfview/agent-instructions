//! `builds rules-on-edit`: the PreToolUse hook of Claude Code's file-editing
//! tools. When an agent edits a file, builds gives the agent each coding rule
//! that covers the file. builds gives an agent each rule once, under the agent's own id (`ack::agent::give_rule`),
//! and gives it again after Claude Code compacts the agent.
//!
//! The rules are those of the project that holds the file's repo, as `builds
//! brief --project P` prints them, or the shared rules when no project holds
//! the repo.

use std::path::{Path, PathBuf};

use crate::ack::agent::{self, HookInput};
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::store::project_of;
use crate::places::Places;
use crate::review::rules::{self, Rule};

const USAGE: &str = "usage: builds rules-on-edit   (a Claude Code PreToolUse hook runs it, with the hook's input on standard input)";

/// What Claude Code's PreToolUse hook passes for an editing tool.
#[derive(serde::Deserialize)]
struct EditInput {
    #[serde(flatten)]
    hook: HookInput,
    #[serde(default)]
    tool_input: ToolInput,
}

/// The arguments of the editing tools that name the file they change: `Edit`,
/// `Write` and `MultiEdit` name it `file_path`, `NotebookEdit` `notebook_path`.
#[derive(Default, serde::Deserialize)]
struct ToolInput {
    file_path: Option<String>,
    notebook_path: Option<String>,
}

pub fn cmd_rules_on_edit(places: &Places, rules_root: &Path, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        fail!("{USAGE}");
    }
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .context(|| "reading the hook's input".to_string())?;
    let input: EditInput = serde_json::from_str(&input).map_err(|error| {
        fail::Fail::Message(format!(
            "the input is not the input of a Claude Code hook: {error}"
        ))
    })?;
    if let Some(output) = on_edit(places, rules_root, &input)? {
        println!("{output}");
    }
    Ok(())
}

/// The hook's output for <input>: the rules that cover the file the tool
/// edits and that builds has not yet given the agent, as context for the
/// agent. None when the tool names no file, the file is in no repo, or the
/// agent holds every rule that covers it.
fn on_edit(places: &Places, rules_root: &Path, input: &EditInput) -> Result<Option<String>> {
    let hook = &input.hook;
    if hook.hook_event_name != "PreToolUse" {
        fail!(
            "`builds rules-on-edit` runs from the PreToolUse hook, not from {}",
            hook.hook_event_name
        );
    }
    let Some(file) = input
        .tool_input
        .file_path
        .as_deref()
        .or(input.tool_input.notebook_path.as_deref())
    else {
        return Ok(None);
    };
    let Some((checkout, path)) = repo_path(Path::new(file))? else {
        return Ok(None);
    };
    let project = project_of(places, &checkout, None)?;
    let rules = match &project {
        Some(project) => rules::load(rules_root, Some(project))?,
        None => rules::load_shared(rules_root)?,
    };
    let (session, agent) = (hook.session_id.as_str(), hook.agent());
    let mut new = Vec::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.matches_paths(std::slice::from_ref(&path)))
    {
        if !agent::was_given_rule(places, session, agent, &rule.slug)? {
            new.push(rule);
        }
    }
    if new.is_empty() {
        return Ok(None);
    }
    let text = render(&path, &new);
    for rule in &new {
        agent::give_rule(places, session, agent, &rule.slug)?;
    }
    Ok(Some(agent::hook_output("PreToolUse", &text)))
}

/// The checkout that holds <file> and <file>'s path in it, or None when it
/// is in no checkout. <file> may not exist yet: its nearest folder that does
/// decides the checkout.
fn repo_path(file: &Path) -> Result<Option<(Checkout, String)>> {
    let mut folder = file.parent();
    let mut rest = PathBuf::from(file.file_name().unwrap_or_default());
    while let Some(candidate) = folder {
        if candidate.is_dir() {
            break;
        }
        rest = PathBuf::from(candidate.file_name().unwrap_or_default()).join(rest);
        folder = candidate.parent();
    }
    let Some(folder) = folder else {
        return Ok(None);
    };
    let Some(checkout) = Checkout::at(folder)? else {
        return Ok(None);
    };
    let real = folder
        .canonicalize()
        .context(|| format!("resolving {}", folder.display()))?;
    let Ok(inside) = real
        .join(&rest)
        .strip_prefix(&checkout.top)
        .map(Path::to_owned)
    else {
        return Ok(None);
    };
    Ok(Some((checkout, inside.display().to_string())))
}

/// What the agent reads for <rules>, the rules that cover <path> and that it
/// has not read.
fn render(path: &str, rules: &[&Rule]) -> String {
    let mut text = format!(
        "Coding rules for `{path}`. builds gives you each rule once, when you first edit a file it covers. Write the edit to follow them.\n"
    );
    for rule in rules {
        text.push_str(&format!(
            "\n### `{}`\n\n{}\n",
            rule.path,
            rule.body().trim_matches('\n')
        ));
    }
    text
}

#[cfg(test)]
#[path = "rules_on_edit_tests.rs"]
mod tests;

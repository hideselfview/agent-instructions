//! `builds rules-on-edit`: the PreToolUse hook of Claude Code's file-editing
//! tools. When an agent edits a file, builds gives the agent each coding rule
//! that covers the file, and beside it the findings of that rule that recent
//! first reviews confirmed most often (`review::frequent`). builds gives an
//! agent each rule once, under the agent's own id (`ack::agent::give_rule`),
//! and gives it again after Claude Code compacts the agent.
//!
//! The rules are those of the project that holds the file's repo, as `builds
//! brief --project P` prints them, or the shared rules when no project holds
//! the repo.

use std::path::{Component, Path, PathBuf};

use crate::ack::agent::{self, HookInput};
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::store::project_of;
use crate::places::Places;
use crate::review::frequent::Frequent;
use crate::review::rules::{self, Rule};

const USAGE: &str = "usage: builds rules-on-edit   (a Claude Code PreToolUse hook runs it, with the hook's input on standard input)";

/// What Claude Code's PreToolUse hook passes for an editing tool.
#[derive(serde::Deserialize)]
struct EditInput {
    #[serde(flatten)]
    hook: HookInput,
    tool_name: String,
    tool_input: ToolInput,
}

/// The arguments of the editing tools that name the file they change: `Edit`,
/// `Write` and `MultiEdit` name it `file_path`, `NotebookEdit` `notebook_path`.
#[derive(serde::Deserialize)]
struct ToolInput {
    file_path: Option<String>,
    notebook_path: Option<String>,
}

impl EditInput {
    /// The file the tool edits. Fails for a tool the hook does not match, and
    /// for a tool call that does not name its file.
    fn file(&self) -> Result<&str> {
        let (field, named) = match self.tool_name.as_str() {
            "Edit" | "Write" | "MultiEdit" => ("file_path", &self.tool_input.file_path),
            "NotebookEdit" => ("notebook_path", &self.tool_input.notebook_path),
            other => fail!(
                "`builds rules-on-edit` runs for Edit, Write, MultiEdit and NotebookEdit, not {other}"
            ),
        };
        match named {
            Some(file) => Ok(file),
            None => fail!("the {} call names no {field}", self.tool_name),
        }
    }
}

/// What the hook gives an agent: the text, and the rules it holds.
struct Delivery {
    output: String,
    slugs: Vec<String>,
}

/// Runs the hook. A failure exits with code 2, which blocks the edit and
/// shows the agent the error: with code 1 the edit would go ahead and the
/// agent would not learn that its rules are missing.
pub fn cmd_rules_on_edit(places: &Places, rules_root: &Path, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        fail!("{USAGE}");
    }
    let input = agent::hook_input_from_stdin()?;
    run(places, rules_root, &input, &mut std::io::stdout()).map_err(|error| {
        crate::text::note(format!(
            "builds could not give you the coding rules for this edit, so the edit did not run: {}",
            error.text()
        ));
        fail::Fail::Exit(2)
    })
}

/// Writes the hook's output for <input> to <out>, then records that the agent
/// holds the rules the output holds, so that a failure before the agent has
/// the text leaves its rules to give again.
fn run(
    places: &Places,
    rules_root: &Path,
    input: &EditInput,
    out: &mut impl std::io::Write,
) -> Result<()> {
    let Some(delivery) = on_edit(places, rules_root, input)? else {
        return Ok(());
    };
    writeln!(out, "{}", delivery.output)
        .and_then(|()| out.flush())
        .context(|| "writing the hook's output".to_string())?;
    let hook = &input.hook;
    for slug in &delivery.slugs {
        agent::give_rule(places, &hook.session_id, hook.agent(), slug)?;
    }
    Ok(())
}

/// What the hook gives <input>'s agent: the rules that cover the file the tool
/// edits and that builds has not yet given the agent, as context for the
/// agent. None when the file is in no repo, or the agent holds every rule that
/// covers it.
fn on_edit(places: &Places, rules_root: &Path, input: &EditInput) -> Result<Option<Delivery>> {
    let hook = &input.hook;
    if hook.hook_event_name != "PreToolUse" {
        fail!(
            "`builds rules-on-edit` runs from the PreToolUse hook, not from {}",
            hook.hook_event_name
        );
    }
    let Some((checkout, path)) = repo_path(Path::new(input.file()?))? else {
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
    let confirmed = match &project {
        Some(project) => Some(crate::review::confirmed_often(places, project, &rules)?),
        None => None,
    };
    Ok(Some(Delivery {
        output: agent::hook_output("PreToolUse", &render(&path, &new, confirmed.as_deref())),
        slugs: new.iter().map(|rule| rule.slug.clone()).collect(),
    }))
}

/// The checkout that holds <file>, an absolute path, and <file>'s path in it,
/// or None when it is in no checkout. <file> may not exist yet: its nearest
/// folder that does decides the checkout.
fn repo_path(file: &Path) -> Result<Option<(Checkout, String)>> {
    if !file.is_absolute()
        || file
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        fail!(
            "{} is not an absolute path with no . or .. in it",
            file.display()
        );
    }
    let name = |path: &Path| match path.file_name() {
        Some(name) => Ok(PathBuf::from(name)),
        None => fail!("{} names no file or folder", path.display()),
    };
    let mut folder = file.parent();
    let mut rest = name(file)?;
    while let Some(candidate) = folder {
        if candidate.is_dir() {
            break;
        }
        rest = name(candidate)?.join(rest);
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
    match real.join(&rest).strip_prefix(&checkout.top) {
        Ok(inside) => Ok(Some((checkout, inside.display().to_string()))),
        Err(_) => fail!(
            "git says {} is in the checkout {}, and its resolved path {} is not",
            folder.display(),
            checkout.top.display(),
            real.display()
        ),
    }
}

/// What the agent reads for <rules>, the rules that cover <path> and that it
/// has not read: each rule's text and, beside it, its entry among the
/// <confirmed> rules when builds counted them (None when no project holds the
/// repo, so builds counted nothing).
fn render(path: &str, rules: &[&Rule], confirmed: Option<&[Frequent]>) -> String {
    let mut text = format!(
        "Coding rules for `{path}`. builds gives you each rule once, when you first edit a file it covers. Write the edit to follow them.\n"
    );
    for rule in rules {
        text.push('\n');
        text.push_str(&rule.printed());
        if let Some(found) =
            confirmed.and_then(|confirmed| confirmed.iter().find(|found| found.rule == rule.slug))
        {
            text.push('\n');
            text.push_str(&found.describe());
        }
    }
    text
}

#[cfg(test)]
#[path = "rules_on_edit_tests.rs"]
mod tests;

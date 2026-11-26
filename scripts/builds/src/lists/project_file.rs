//! A project's file in the rules checkout (projects/<project>.md): the
//! description every agent of the project starts with, and its hard limits.

use std::path::Path;

use crate::fail;
use crate::fail::{Context, Result};

/// The heading of the section that holds a project's hard limits.
const LIMITS_HEADING: &str = "## Hard limits";

/// What builds reads from projects/<project>.md.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFile {
    /// The file's path, for messages.
    pub path: String,
    /// The first paragraph after the title.
    pub description: String,
    /// The text of the `## Hard limits` section, without its heading.
    pub hard_limits: String,
}

/// Reads <project>'s file in the checkout at <rules_root>. Fails, naming the
/// file, when it is missing, has no description or has no Hard limits section.
pub fn load(rules_root: &Path, project: &str) -> Result<ProjectFile> {
    let path = rules_root.join("projects").join(format!("{project}.md"));
    let shown = path.display().to_string();
    if !path.is_file() {
        fail!("project {project} has no file {shown}, which holds its description and hard limits");
    }
    let text = std::fs::read_to_string(&path).context(|| format!("reading {shown}"))?;
    let Some(description) = description(&text) else {
        fail!("{shown} has no description: its first paragraph after the title is missing");
    };
    let Some(hard_limits) = hard_limits(&text) else {
        fail!(
            "{shown} has no `{LIMITS_HEADING}` section, which agents of project {project} start with"
        );
    };
    Ok(ProjectFile {
        path: shown,
        description,
        hard_limits,
    })
}

/// The text of <project>'s Hard limits section, None when the project has no
/// file or no such section. The pasted-text check needs neither to exist.
pub fn hard_limits_if_any(rules_root: &Path, project: &str) -> Option<String> {
    let text =
        std::fs::read_to_string(rules_root.join("projects").join(format!("{project}.md"))).ok()?;
    hard_limits(&text)
}

/// The first paragraph after the title heading, its lines joined with
/// newlines, None when a heading or the end of the file comes first.
fn description(text: &str) -> Option<String> {
    let mut lines = text.lines().skip_while(|line| line.trim().is_empty());
    if !lines.next()?.starts_with("# ") {
        return None;
    }
    let paragraph: Vec<&str> = lines
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty())
        .collect();
    if paragraph.is_empty() || paragraph[0].starts_with('#') {
        return None;
    }
    Some(paragraph.join("\n"))
}

/// The lines between `## Hard limits` and the next `## ` heading, trimmed,
/// None when the heading or its body is missing.
fn hard_limits(text: &str) -> Option<String> {
    let mut lines = text.lines();
    lines.find(|line| line.trim_end() == LIMITS_HEADING)?;
    let body: Vec<&str> = lines.take_while(|line| !line.starts_with("## ")).collect();
    let body = body.join("\n").trim().to_string();
    (!body.is_empty()).then_some(body)
}

#[cfg(test)]
#[path = "project_file_tests.rs"]
mod tests;

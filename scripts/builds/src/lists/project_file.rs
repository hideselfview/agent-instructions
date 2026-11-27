//! A project's file in the rules checkout (projects/<project>.md): the
//! description every agent of the project starts with, its hard limits and the
//! folders no review reads.

use std::collections::BTreeMap;
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
/// file or no such section. The pasted-text check needs neither to exist; any
/// other failure to read the file is an error.
pub fn hard_limits_if_any(rules_root: &Path, project: &str) -> Result<Option<String>> {
    let path = rules_root.join("projects").join(format!("{project}.md"));
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(hard_limits(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}

/// The heading of the section that names the folders no review reads.
const UNREAD_HEADING: &str = "## Paths no review reads";

/// Per repo, the folders that the `## Paths no review reads` section of
/// <text>, the project file at <path>, names, each as a line
/// ``- `repo:folder/` ``. Other lines of the section are prose. Empty when the
/// file has no such section. Fails on a list line in any other form, and on a
/// section that names no folder.
pub fn unread_folders(path: &Path, text: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let mut folders: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Some(lines) = section(text, UNREAD_HEADING) else {
        return Ok(folders);
    };
    for line in lines {
        let Some(entry) = line.strip_prefix("- ") else {
            continue;
        };
        let Some((repo, folder)) = unread_entry(entry.trim_end()) else {
            fail!(
                "{}: `{line}` under `{UNREAD_HEADING}` is not a line of the form - `repo:folder/`",
                path.display()
            );
        };
        folders
            .entry(repo.to_string())
            .or_default()
            .push(folder.to_string());
    }
    if folders.is_empty() {
        fail!(
            "{}: `{UNREAD_HEADING}` names no folder; name each as - `repo:folder/`, or remove the section",
            path.display()
        );
    }
    Ok(folders)
}

/// The repo and the folder, without its last slash, of `` `repo:folder/` ``.
fn unread_entry(entry: &str) -> Option<(&str, &str)> {
    let entry = entry.strip_prefix('`')?.strip_suffix('`')?;
    let (repo, folder) = entry.split_once(':')?;
    let folder = folder.strip_suffix('/')?;
    let is_folder = folder
        .split('/')
        .all(|part| !matches!(part, "" | "." | ".."));
    (!repo.is_empty() && is_folder).then_some((repo, folder))
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

/// The lines of <text> between the line <heading> and the next `## ` heading,
/// None when <text> has no such heading.
fn section<'a>(text: &'a str, heading: &str) -> Option<impl Iterator<Item = &'a str>> {
    let mut lines = text.lines();
    lines.find(|line| line.trim_end() == heading)?;
    Some(lines.take_while(|line| !line.starts_with("## ")))
}

/// The lines of the `## Hard limits` section, trimmed, None when the heading
/// or its body is missing.
fn hard_limits(text: &str) -> Option<String> {
    let body: Vec<&str> = section(text, LIMITS_HEADING)?.collect();
    let body = body.join("\n").trim().to_string();
    (!body.is_empty()).then_some(body)
}

#[cfg(test)]
#[path = "project_file_tests.rs"]
mod tests;

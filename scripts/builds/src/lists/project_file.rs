//! A project's file in the rules checkout (projects/<project>.md): its hard
//! limits, which builds quotes to every agent of the project and refuses a
//! brief to repeat.

use std::path::Path;

/// The heading of the section that holds a project's hard limits.
const LIMITS_HEADING: &str = "## Hard limits";

/// The text of <project>'s Hard limits section in the checkout at
/// <rules_root>, None when the project has no file or no such section.
pub fn hard_limits_if_any(rules_root: &Path, project: &str) -> Option<String> {
    let text =
        std::fs::read_to_string(rules_root.join("projects").join(format!("{project}.md"))).ok()?;
    hard_limits(&text)
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

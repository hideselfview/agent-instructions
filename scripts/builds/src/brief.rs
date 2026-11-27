//! Prints the instructions a planner or implementing agent needs for its paths.

use std::io::Write;
use std::path::{Component, Path};

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::Args;
use crate::lists::check::project_name_fault;
use crate::review::rules;

pub fn cmd_brief(root: &Path, args: &[String]) -> Result<()> {
    let args = Args::parse_only(args, &["project"])?;
    let paths = paths(&args)?;
    let project = args.value("project")?;
    if let Some(fault) = project.and_then(project_name_fault) {
        fail!("{fault}");
    }
    let rules = match project {
        Some(project) => rules::load_project(root, project)?,
        None => rules::load_shared(root)?,
    };
    crate::output::answer_handle()
        .lock()
        .write_all(render(&paths, &rules).as_bytes())
        .context(|| "writing the brief".to_string())
}

fn paths(args: &Args) -> Result<Vec<String>> {
    if args.positional().is_empty() {
        fail!("{}", crate::command::Command::Brief.usage_text());
    }
    args.positional()
        .iter()
        .map(|arg| {
            let mut names = Vec::new();
            for component in Path::new(arg).components() {
                match component {
                    Component::CurDir => {}
                    Component::Normal(name) => names.push(name.to_string_lossy()),
                    _ => fail!("{arg} must name a file relative to the repo root"),
                }
            }
            if names.is_empty() {
                fail!("{arg:?} must name a file relative to the repo root");
            }
            Ok(names.join("/"))
        })
        .collect()
}

fn render(paths: &[String], rules: &[rules::Rule]) -> String {
    let mut text = String::from("## Coding rules\n");
    for rule in rules.iter().filter(|rule| rule.matches_paths(paths)) {
        text.push('\n');
        text.push_str(&rule.printed());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brief_rejects_the_by_flag() {
        let scratch = crate::testing::Scratch::new("brief-flags");
        let args = ["--by".into(), "agent".into(), "src/main.rs".into()];
        let error = cmd_brief(&scratch.dir, &args).unwrap_err().text();
        assert_eq!(error, "unknown flag --by; this command takes --project");
    }

    #[test]
    fn a_brief_includes_full_authoring_rules_once() {
        let mut shared = rules::Rule::parse(
            "authoring",
            "---\npaths: ['**/*.rs']\nreview: false\n---\nAuthoring guidance.\n\nAnother paragraph.\n",
        );
        shared.path = "rules/authoring.md".into();
        let mut project = rules::Rule::parse(
            "project",
            "---\npaths: ['src/**']\n---\nProject guidance.\n",
        );
        project.path = "projects/example-rules/project.md".into();
        let unrelated = rules::Rule::parse(
            "swift",
            "---\npaths: ['**/*.swift']\n---\nSwift guidance.\n",
        );
        let printed = render(
            &["src/main.rs".into(), "src/tests.rs".into()],
            &[shared, project, unrelated],
        );
        assert_eq!(
            printed,
            "## Coding rules\n\n### `rules/authoring.md`\n\nAuthoring guidance.\n\nAnother paragraph.\n\n### `projects/example-rules/project.md`\n\nProject guidance.\n"
        );
    }
}

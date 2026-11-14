//! `builds lists init | add-repo | show | project-of`.

use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::Args;
use crate::lists::check::repo_name_fault;
use crate::lists::project::{self, NewProject, parse_globs};
use crate::lists::store::{Store, project_of, projects_holding};
use crate::lists::view::View;
use crate::lists::{Stamp, show};
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds lists init PROJECT --repos R,R [--design-places R:GLOB,...] [--design-sources R:GLOB,...] [--spec-files R:GLOB,...] [--by WHO]\n       builds lists add-repo PROJECT REPO [--by WHO]\n       builds lists show [--json] [--project P]\n       builds lists project-of DIR";

pub fn cmd_lists(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = std::env::current_dir().context(|| "reading the current directory".to_string())?;
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "init" => init(tool, rest, &cwd),
        "add-repo" => add_repo(tool, rest, &cwd),
        "show" => show(tool, rest, &cwd),
        "project-of" => project_of_command(tool, rest),
        other => fail!("unknown lists command {other}\n{USAGE}"),
    }
}

fn init(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(
        args,
        &["repos", "design-places", "design-sources", "spec-files"],
        &[],
    )?;
    let [project] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let Some(repos) = args
        .value("repos")?
        .map(|r| r.split(',').map(str::to_string).collect::<Vec<String>>())
    else {
        fail!("init needs --repos R,R: the repos the project's lists cover");
    };
    let globs = |flag| {
        args.value(flag)
            .and_then(|text| text.map(|t| parse_globs(flag, t)).transpose())
    };
    let new = NewProject {
        name: project.to_string(),
        repos,
        places: globs("design-places")?.unwrap_or_default(),
        sources: globs("design-sources")?.unwrap_or_default(),
        specs: globs("spec-files")?.unwrap_or_default(),
    };
    let store = Store::named(&tool.places, project)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.create(&new, &stamp, &format!("lists init {project}"))?;
    note(format!(
        "made the lists of project {project} in {}",
        store.dir.display()
    ));
    Ok(())
}

/// `lists add-repo PROJECT REPO`: the project carries the work of one more
/// repo. A repo is in one project, so one another project holds is refused.
fn add_repo(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [project, repo] = args.exactly(2, USAGE)? else {
        unreachable!("exactly two")
    };
    if let Some(fault) = repo_name_fault(repo) {
        fail!("{fault}");
    }
    let store = Store::named(&tool.places, project)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("lists add-repo {project} {repo}"), |db| {
        if let Some(other) = projects_holding(&tool.places, repo)?
            .into_iter()
            .find(|held| held != project)
        {
            fail!("repo {repo} is already in project {other}; a repo is in one project");
        }
        project::add_repo(db, repo)
    })?;
    note(format!("project {project} now carries repo {repo}"));
    Ok(())
}

fn show(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &["json"])?;
    args.exactly(0, USAGE)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let lists = store.load()?;
    if args.switch("json") {
        let view = View::build(&lists, tool.now);
        let text =
            serde_json::to_string_pretty(&view).context(|| "writing the lists JSON".to_string())?;
        println!("{text}");
    } else {
        print!("{}", show::render(&lists));
    }
    Ok(())
}

fn project_of_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [dir] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let checkout = Checkout::find(dir)?;
    if let Some(project) = project_of(&tool.places, &checkout)? {
        println!("{project}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;

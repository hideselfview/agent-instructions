//! `builds lists init | add-repo | show | project-of`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::args::{Args, read_input};
use crate::lists::check::repo_name_fault;
use crate::lists::store::{Store, project_of, projects_holding};
use crate::lists::view::View;
use crate::lists::{Lists, Stamp, show};
use crate::mode;
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds lists init PROJECT --repos R,R [--design-places R:GLOB,...] [--design-sources R:GLOB,...] [--spec-files R:GLOB,...] [--from FILE] [--by WHO]\n       builds lists add-repo PROJECT REPO [--by WHO]\n       builds lists show [--json] [--project P]\n       builds lists project-of DIR";

pub fn cmd_lists(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = std::env::current_dir().context(|| "reading the current directory".to_string())?;
    let Some((command, rest)) = args.split_first() else { fail!("{USAGE}") };
    match command.as_str() {
        "init" => init(tool, rest, &cwd),
        "add-repo" => add_repo(tool, rest, &cwd),
        "show" => show(tool, rest, &cwd),
        "project-of" => project_of_command(tool, rest),
        other => fail!("unknown lists command {other}\n{USAGE}"),
    }
}

/// `R:GLOB,R:GLOB` as each repo's globs, for the flag <flag>.
fn parse_globs(flag: &str, text: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let mut places: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for part in text.split(',') {
        let Some((repo, glob)) = part.split_once(':') else {
            fail!("--{flag} takes REPO:GLOB entries, got '{part}'");
        };
        places.entry(repo.to_string()).or_default().push(glob.to_string());
    }
    Ok(places)
}

fn init(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["repos", "design-places", "design-sources", "spec-files", "from"], &[])?;
    let [project] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    let repos: Option<Vec<String>> = args.value("repos")?.map(|r| r.split(',').map(str::to_string).collect());
    let globs = |flag| args.value(flag).and_then(|text| text.map(|t| parse_globs(flag, t)).transpose());
    let design_places = globs("design-places")?;
    let design_sources = globs("design-sources")?;
    let spec_files = globs("spec-files")?;
    let lists = match args.value("from")? {
        None => {
            let Some(repos) = repos else { fail!("init needs --repos R,R: the repos the project's lists cover") };
            let mut lists = Lists::empty(project, repos, design_places.unwrap_or_default());
            lists.design_sources = design_sources.unwrap_or_default();
            lists.spec_files = spec_files.unwrap_or_default();
            lists
        }
        Some(from) => {
            let text = read_input(from)?;
            let lists: Lists = serde_json::from_str(&text)
                .map_err(|error| Fail::Message(format!("{from} is not valid lists: {error}")))?;
            if lists.project != *project {
                fail!("{from} holds the lists of project {}, not {project}", lists.project);
            }
            if repos.as_ref().is_some_and(|given| *given != lists.repos) {
                fail!("--repos does not match the repos {from} holds ({})", lists.repos.join(","));
            }
            for (flag, given, held) in [
                ("design-places", &design_places, &lists.design_places),
                ("design-sources", &design_sources, &lists.design_sources),
                ("spec-files", &spec_files, &lists.spec_files),
            ] {
                if given.as_ref().is_some_and(|given| given != held) {
                    fail!("--{flag} does not match the globs {from} holds");
                }
            }
            lists
        }
    };
    let store = Store::named(&tool.places, project)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let words = match args.value("from")? {
        Some(from) => format!("lists init {project} --from {from}"),
        None => format!("lists init {project}"),
    };
    store.create(lists, &stamp, &words)?;
    note(format!("made the lists of project {project} in {}", store.dir.display()));
    Ok(())
}

/// `lists add-repo PROJECT REPO`: the project carries the work of one more
/// repo. A repo is in one project, so one another project holds is refused.
fn add_repo(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [project, repo] = args.exactly(2, USAGE)? else { unreachable!("exactly two") };
    if let Some(fault) = repo_name_fault(repo) {
        fail!("{fault}");
    }
    let store = Store::named(&tool.places, project)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("lists add-repo {project} {repo}"), |lists| {
        if lists.repos.contains(repo) {
            fail!("repo {repo} is already one of project {project}'s");
        }
        if let Some(other) = projects_holding(&tool.places, repo)?.into_iter().find(|held| held != project) {
            fail!("repo {repo} is already in project {other}; a repo is in one project");
        }
        lists.repos.push(repo.clone());
        Ok(())
    })?;
    note(format!("project {project} now carries repo {repo}"));
    Ok(())
}

fn show(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &["json"])?;
    args.exactly(0, USAGE)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let lists = store.load()?;
    let mode = mode::current(&tool.places)?;
    if args.switch("json") {
        let view = View::build(&lists, tool.now, mode);
        let text = serde_json::to_string_pretty(&view).context(|| "writing the lists JSON".to_string())?;
        println!("{text}");
    } else {
        print!("{}", show::render(&lists, mode));
    }
    Ok(())
}

fn project_of_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [dir] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    let checkout = Checkout::find(dir)?;
    if let Some(project) = project_of(&tool.places, &checkout)? {
        println!("{project}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;

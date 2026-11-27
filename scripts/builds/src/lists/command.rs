//! `builds lists init | add-repo | show | project-of`.

use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::Args;
use crate::lists::check::repo_name_fault;
use crate::lists::project::{self, GlobKind, NewProject, parse_globs};
use crate::lists::store::{Store, project_of};
use crate::lists::view::View;
use crate::lists::{Stamp, show};
use crate::output::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds lists init PROJECT --repos R,R [--canvas yes|no] [--design-places R:GLOB,...] [--design-sources R:GLOB,...] [--spec-files R:GLOB,...] [--by WHO]\n       builds lists add-repo PROJECT REPO [--by WHO]\n       builds lists design-globs PROJECT [--canvas yes|no] [--design-places R:GLOB,...] [--design-sources R:GLOB,...] [--spec-files R:GLOB,...] [--by WHO]\n       builds lists show [--json | --tag T] [--project P]\n       builds lists project-of DIR";

pub fn cmd_lists(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = std::env::current_dir().context(|| "reading the current directory".to_string())?;
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "init" => init(tool, rest, &cwd),
        "add-repo" => add_repo(tool, rest, &cwd),
        "design-globs" => design_globs(tool, rest, &cwd),
        "show" => show(tool, rest, &cwd),
        "project-of" => project_of_command(tool, rest),
        other => fail!("unknown lists command {other}\n{USAGE}"),
    }
}

fn init(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(
        args,
        &[
            "repos",
            "canvas",
            "design-places",
            "design-sources",
            "spec-files",
        ],
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
        canvas: canvas(&args)?.unwrap_or(false),
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
/// repo. Two projects may hold the same repo (agent-instructions is in bae
/// and builds); a worktree of such a repo says which project its work is in
/// (`builds claim --project`).
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
        project::add_repo(db, repo)
    })?;
    note(format!("project {project} now carries repo {repo}"));
    Ok(())
}

/// `--canvas yes|no`: whether the project publishes its boards as a
/// claude.ai canvas; None when the caller leaves the flag out.
fn canvas(args: &Args) -> Result<Option<bool>> {
    match args.value("canvas")? {
        None => Ok(None),
        Some("yes") => Ok(Some(true)),
        Some("no") => Ok(Some(false)),
        Some(other) => fail!("--canvas is yes or no, not {other:?}"),
    }
}

/// `lists design-globs PROJECT`: sets the design globs of each kind given,
/// for each repo that kind names, and whether the project publishes its
/// boards as a canvas, in a store that exists.
fn design_globs(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(
        args,
        &["canvas", "design-places", "design-sources", "spec-files"],
        &[],
    )?;
    let [project] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let mut given = Vec::new();
    for (flag, kind) in [
        ("design-places", GlobKind::Place),
        ("design-sources", GlobKind::Source),
        ("spec-files", GlobKind::Spec),
    ] {
        if let Some(text) = args.value(flag)? {
            given.push((kind, parse_globs(flag, text)?));
        }
    }
    let canvas = canvas(&args)?;
    if given.is_empty() && canvas.is_none() {
        fail!(
            "design-globs needs --canvas, --design-places, --design-sources or --spec-files\n{USAGE}"
        );
    }
    let store = Store::named(&tool.places, project)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("lists design-globs {project}"), |db| {
        if let Some(canvas) = canvas {
            project::set_canvas(db, canvas)?;
        }
        given
            .iter()
            .try_for_each(|(kind, globs)| project::set_design_globs(db, *kind, globs))
    })?;
    note(format!("set the design globs of project {project}"));
    Ok(())
}

fn show(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["tag"], &["json"])?;
    args.exactly(0, USAGE)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let lists = store.load()?;
    let wanted = args.value("tag")?;
    if args.switch("json") {
        if wanted.is_some() {
            fail!(
                "--tag chooses what the text shows; --json prints the whole state, with each record's tags"
            );
        }
        let view = View::build(&lists, tool.now)?;
        let text =
            serde_json::to_string_pretty(&view).context(|| "writing the lists JSON".to_string())?;
        crate::output::answer_line(text);
    } else {
        crate::output::answer(show::render(&lists, wanted)?);
    }
    Ok(())
}

fn project_of_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [dir] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let checkout = Checkout::find(dir)?;
    if let Some(project) = project_of(&tool.places, &checkout, None)? {
        crate::output::answer_line(project);
    }
    Ok(())
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;

//! `builds app-build <repo> <ref> [--test <Class>]... [--shots <prefix>]...
//! [--fresh]` with no step named: builds <ref> in the repo's app checkouts,
//! choosing for itself what to build and where, so the caller names no step,
//! lane or host (`builds app-build <repo> <ref> <step>...` names steps).
//!
//! What to build comes from the branch's diff: the paths that differ between
//! <ref> and its merge-base with `origin/main` each match the `on` lines of
//! the repo's `.app-build` (app_config.rs), which name the steps they need. A
//! branch that changes no path an `on` line matches builds nothing.
//!
//! A `--test <Class>` run goes to the lane whose `sources` folders declare the
//! class at <ref>; a `--shots <prefix>` run goes to the lane its prefix starts
//! with (`mac.welcome.`). A lane with such a run builds what its changed paths
//! need, or, when they need nothing in that lane, what its `needs` line
//! names, so the run has a build of <ref> to run against. A `--shots` run
//! whose findings for <ref> are already stored prints them instead of running,
//! unless `--fresh`.
//!
//! The lane's process (app_build.rs) then picks the checkout: the free one on
//! the less loaded host, `here`-only steps staying on this Mac (lock.rs).

use std::collections::HashSet;
use std::path::Path;

use crate::app_build::{self, Pick};
use crate::app_config::{CONFIG, Config, PickKind, run_git};
use crate::checkout::git;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::text::note;

const USAGE: &str =
    "usage: builds app-build <repo> <ref> [--test <Class>]... [--shots <prefix>]... [--fresh]";

/// The steps each lane builds: the steps of every `on` line that matches a
/// path of <changed>, in the order the steps are written, and for a lane with
/// a run among <picks> and no such step, the steps of its `needs` lines. A
/// lane with neither is left out.
pub fn plan(config: &Config, changed: &[String], picks: &[Pick]) -> Vec<(Lane, Vec<String>)> {
    let wanted: HashSet<&str> = config
        .rules
        .iter()
        .filter(|rule| changed.iter().any(|path| rule.matches(path)))
        .flat_map(|rule| rule.steps.iter().map(String::as_str))
        .collect();
    Lane::ALL
        .into_iter()
        .map(|lane| {
            let picked = picks.iter().any(|(own, _, _)| *own == lane);
            let mut names: Vec<String> = config
                .steps
                .iter()
                .filter(|step| step.lane == lane && wanted.contains(step.name.as_str()))
                .map(|step| step.name.clone())
                .collect();
            if names.is_empty() && picked {
                let needed: Vec<&String> = config
                    .needs
                    .iter()
                    .filter(|(own, _)| *own == lane)
                    .flat_map(|(_, steps)| steps)
                    .collect();
                names = config
                    .steps
                    .iter()
                    .filter(|step| needed.contains(&&step.name))
                    .map(|step| step.name.clone())
                    .collect();
            }
            (lane, names, picked)
        })
        .filter(|(_, names, picked)| !names.is_empty() || *picked)
        .map(|(lane, names, _)| (lane, names))
        .collect()
}

/// The paths that differ between <commit> and its merge-base with
/// `origin/main`, relative to the repo's top.
pub fn changed_paths(top: &Path, commit: &str) -> Result<Vec<String>> {
    let base = run_git(top, &["merge-base", commit, "origin/main"])?;
    let diff = run_git(top, &["diff", "--name-only", base.trim(), commit])?;
    Ok(diff.lines().map(str::to_string).collect())
}

/// The lanes whose `sources` folders hold a file at <commit> that has <class>
/// as a whole word.
fn lanes_declaring(top: &Path, config: &Config, commit: &str, class: &str) -> Result<Vec<Lane>> {
    let mut found = Vec::new();
    for lane in Lane::ALL {
        let folders: Vec<&str> = config
            .sources
            .iter()
            .filter(|(own, _)| *own == lane)
            .map(|(_, folder)| folder.as_str())
            .collect();
        if folders.is_empty() {
            continue;
        }
        let output = git(top)
            .args(["grep", "-l", "-w", "-F", "-e", class, commit, "--"])
            .args(&folders)
            .output()
            .context(|| "running git to search the sources".to_string())?;
        match output.status.code() {
            Some(0) => found.push(lane),
            Some(1) => {}
            _ => fail!(
                "git failed to search the {} sources: {}",
                lane.word(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }
    Ok(found)
}

/// The lane of the class <class>, from the <declaring> lanes.
fn lane_of_class(declaring: &[Lane], class: &str) -> Result<Lane> {
    match declaring {
        [lane] => Ok(*lane),
        [] => fail!(
            "no file in the sources lines' folders of {CONFIG} has the class {class}, so no lane runs it"
        ),
        _ => fail!(
            "the class {class} appears in both lanes' sources; name it by a longer, unique word"
        ),
    }
}

/// The lane a shots <prefix> starts with (`mac.welcome.` is the mac lane's).
fn lane_of_prefix(prefix: &str) -> Result<Lane> {
    let first = prefix.split('.').next().unwrap_or("");
    match Lane::parse(first) {
        Some(lane) => Ok(lane),
        None => fail!(
            "the shots prefix {prefix:?} starts with no lane; shots names start with mac. or android."
        ),
    }
}

/// <args> after the repo and the ref: the picks, each with the lane it runs
/// in, and whether `--fresh` was given.
fn parse_picks(
    args: &[String],
    mut lane_of: impl FnMut(PickKind, &str) -> Result<Lane>,
) -> Result<(Vec<Pick>, bool)> {
    let (mut picks, mut fresh) = (Vec::new(), false);
    let mut rest = args;
    while let Some((flag, after)) = rest.split_first() {
        if flag == "--fresh" {
            fresh = true;
            rest = after;
            continue;
        }
        let Some(kind) = PickKind::ALL.into_iter().find(|kind| flag == kind.flag()) else {
            fail!("{flag:?} where --test, --shots or --fresh goes\n{USAGE}");
        };
        let [arg, after @ ..] = after else {
            fail!("{} needs a {}\n{USAGE}", kind.flag(), kind.what());
        };
        if arg.is_empty() || arg.starts_with('-') || arg.contains(char::is_whitespace) {
            fail!(
                "{} takes one {} name, not {arg:?}",
                kind.flag(),
                kind.what()
            );
        }
        picks.push((lane_of(kind, arg)?, kind, arg.clone()));
        rest = after;
    }
    Ok((picks, fresh))
}

pub fn derive_and_run(places: &Places, args: &[String]) -> Result<()> {
    let [repo, git_ref, tail @ ..] = args else {
        fail!("{USAGE}");
    };
    if git_ref.starts_with('-') {
        fail!("{USAGE}");
    }
    let (top, commit, config) = app_build::commit_and_config(places, repo, git_ref)?;
    let (mut picks, fresh) = parse_picks(tail, |kind, arg| match kind {
        PickKind::Shots => lane_of_prefix(arg),
        PickKind::Class => lane_of_class(&lanes_declaring(&top, &config, &commit, arg)?, arg),
    })?;
    for (lane, kind, arg) in &picks {
        if config.picker(*lane, *kind).is_none() {
            return Err(app_build::missing_line(*lane, *kind, arg));
        }
    }
    if !picks.is_empty() && !fresh {
        picks = app_build::serve_stored(places, repo, &commit, &config, picks)?;
        if picks.is_empty() {
            return Ok(());
        }
    }
    let lanes = plan(&config, &changed_paths(&top, &commit)?, &picks);
    if lanes.is_empty() {
        note(format!(
            "nothing to build: no path {git_ref} changes needs a step ({CONFIG}'s `on` lines)"
        ));
        return Ok(());
    }
    for (lane, names) in &lanes {
        note(format!(
            "{} lane: {}",
            lane.word(),
            if names.is_empty() {
                "no build, only the runs asked for".to_string()
            } else {
                names.join(" ")
            }
        ));
    }
    app_build::run_lanes(repo, &commit, &lanes, &picks)
}

#[cfg(test)]
#[path = "derive_tests.rs"]
mod tests;

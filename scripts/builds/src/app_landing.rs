//! `builds app-build --check-landing <worktree> <before> <result>`: whether
//! the passing app builds recorded in the store built what a landing puts on
//! main. `builds land` runs it on the result it built, once it has picked the
//! commits onto origin/main (<before>) and before it pushes (<result>).
//!
//! Each lane's `on` lines in the result's `.app-build` (app_config.rs) name the
//! paths whose change the lane's apps compile. The landing changes a lane's
//! apps when a path that differs between <before> and <result> matches one of
//! the lane's `on` lines. For each such lane, a passing run of the lane covers
//! the landing when it passed every step the landing's change needs in the
//! lane, and the commit it built has the result's files at every path the
//! lane's `on` lines match. A lane that no passing run covers refuses the
//! landing. So when another landing that changed the lane reached main after
//! the branch's last app build, the result differs from that build at the
//! other landing's paths, and the branch builds again on the new main before
//! it lands. Land runs no app build itself.
//!
//! The check reads the latest passing runs of each lane
//! (`app_timing::PASSED_RUNS_READ`), newest first, and stops at the first
//! that covers the landing. A run whose commit git does not have in the
//! worktree's repository covers nothing, and the check says so.

use std::path::Path;

use crate::app_config::{CONFIG, Config, parse_config, run_git};
use crate::derive::paths_between;
use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;
use crate::places::Places;

const USAGE: &str = "usage: builds app-build --check-landing <worktree> <before> <result>";

/// The paths of <paths> that one of <lane>'s `on` lines in <config> matches:
/// a line that names a step of the lane.
fn lane_paths<'a>(config: &Config, lane: Lane, paths: &'a [String]) -> Vec<&'a str> {
    let in_lane = |name: &String| {
        config
            .steps
            .iter()
            .any(|step| step.lane == lane && step.name == *name)
    };
    paths
        .iter()
        .filter(|path| {
            config
                .rules
                .iter()
                .any(|rule| rule.matches(path) && rule.steps.iter().any(in_lane))
        })
        .map(String::as_str)
        .collect()
}

/// A lane whose apps the landing changes and that no passing run covers,
/// with the newest passing run git has and the paths where its commit
/// differs from the result, or None when the lane has no such run.
#[derive(Debug, PartialEq, Eq)]
pub struct Uncovered {
    pub lane: Lane,
    pub newest: Option<(String, Vec<String>)>,
}

/// The lanes that <changed>, the paths the landing changes, changes in
/// <config> and that no passing run covers. <passed> gives a lane's latest
/// passing runs, newest first, and <differ> the paths where a commit differs
/// from the result, or None when git does not have the commit.
pub fn uncovered(
    config: &Config,
    changed: &[String],
    passed: &mut dyn FnMut(Lane) -> Result<Vec<crate::app_timing::PassedRun>>,
    differ: &mut dyn FnMut(&str) -> Result<Option<Vec<String>>>,
) -> Result<Vec<Uncovered>> {
    let mut found = Vec::new();
    for lane in Lane::ALL {
        if lane_paths(config, lane, changed).is_empty() {
            continue;
        }
        let needed: Vec<String> = crate::derive::plan(config, changed, &[])
            .into_iter()
            .filter(|(own, _)| *own == lane)
            .flat_map(|(_, steps)| steps)
            .collect();
        let mut newest = None;
        let mut covered = false;
        for run in passed(lane)? {
            let Some(differs) = differ(&run.commit)? else {
                continue;
            };
            let paths: Vec<String> = lane_paths(config, lane, &differs)
                .into_iter()
                .map(str::to_string)
                .collect();
            if paths.is_empty() && needed.iter().all(|step| run.steps.contains(step)) {
                covered = true;
                break;
            }
            if newest.is_none() {
                newest = Some((run.commit, paths));
            }
        }
        if !covered {
            found.push(Uncovered { lane, newest });
        }
    }
    Ok(found)
}

/// What land prints when no passing run covers some lanes of the landing of
/// <repo> from <worktree>.
fn refusal(repo: &str, worktree: &str, lanes: &[Uncovered]) -> String {
    let mut said = format!(
        "the result changes what the apps of {repo} compile, and no passing app build built the result's files for these lanes:"
    );
    for Uncovered { lane, newest } in lanes {
        let lane = lane.word();
        match newest {
            None => said.push_str(&format!(
                "\n  {lane}: the store records no passing {lane} app build of a commit git has"
            )),
            Some((commit, paths)) if paths.is_empty() => said.push_str(&format!(
                "\n  {lane}: the newest passing {lane} app build, of {}, did not pass every step the change needs",
                &commit[..commit.len().min(12)]
            )),
            Some((commit, paths)) => {
                let shown: Vec<&str> = paths.iter().take(10).map(String::as_str).collect();
                let more = if paths.len() > shown.len() {
                    format!(" and {} more", paths.len() - shown.len())
                } else {
                    String::new()
                };
                said.push_str(&format!(
                    "\n  {lane}: the newest passing {lane} app build, of {}, has other files than the result at {}{more}",
                    &commit[..commit.len().min(12)],
                    shown.join(", ")
                ));
            }
        }
    }
    said.push_str(&format!(
        "\nNothing landed. Next: in {worktree}, run `git fetch origin && git rebase origin/main` on the branch, run `builds app-build {repo} <branch>` with the branch's test classes and shots, review the branch again when the rebase changed its lines, and land again."
    ));
    said
}

/// `builds app-build --check-landing <worktree> <before> <result>`: prints the
/// refusal and fails when no passing run covers some lane of the landing. A
/// repo in no project records no runs, and a result with no `.app-build`
/// builds no apps, so neither refuses anything.
pub fn cmd_check_landing(places: &Places, args: &[String]) -> Result<()> {
    let [worktree, before, result] = args else {
        fail!("{USAGE}");
    };
    let top = Path::new(worktree);
    let Some(checkout) = crate::checkout::Checkout::at(top)? else {
        fail!("git finds no checkout at {worktree}");
    };
    if run_git(top, &["ls-tree", "--name-only", result, "--", CONFIG])?.is_empty() {
        crate::text::note(format!(
            "{result} has no {CONFIG}, so it builds no apps and no app build has to cover it"
        ));
        return Ok(());
    }
    let repo = checkout.repo();
    let Some(database) = crate::app_timing::database_of(places, &repo)? else {
        crate::text::note(format!(
            "repo {repo} is in no project, so no store records its app builds and builds checks none"
        ));
        return Ok(());
    };
    let config = parse_config(&run_git(top, &["show", &format!("{result}:{CONFIG}")])?)?;
    let changed = paths_between(top, before, result)?;
    let conn = crate::db::open(&database, crate::db::Make::Never)?;
    let lanes = uncovered(
        &config,
        &changed,
        &mut |lane| crate::app_timing::passed_runs(&conn, &repo, lane, &database),
        &mut |commit| {
            if !crate::review::commits::exists(top, commit)? {
                crate::text::note(format!(
                    "a passing app build built {commit}, which git does not have in {worktree}, so that build covers nothing"
                ));
                return Ok(None);
            }
            paths_between(top, commit, result).map(Some)
        },
    )?;
    if lanes.is_empty() {
        return Ok(());
    }
    println!("{}", refusal(&repo, worktree, &lanes));
    Err(crate::fail::Fail::Exit(1))
}

#[cfg(test)]
#[path = "app_landing_tests.rs"]
mod tests;

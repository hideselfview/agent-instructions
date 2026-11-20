//! `builds app-build --landing-build <worktree> <before> <result> <branch>`:
//! the app build that a landing needs before its push. `builds land` runs it
//! once it has picked the commits onto origin/main (<before>) into <result>,
//! with <branch>, the last commit it lands as the branch holds it.
//!
//! Each lane's `on` lines in the result's `.app-build` (app_config.rs) name the
//! paths whose change the lane's apps compile. The landing changes a lane's
//! apps when a path that differs between <before> and <result> matches one of
//! the lane's `on` lines. A passing run of the lane covers a commit when it
//! passed every step the landing's change needs in the lane, and the commit it
//! built has that commit's files at every path the lane's `on` lines match.
//!
//! A lane that a passing run covers at the result needs no build. Otherwise
//! the lane needs the branch's own build: the newest passing run of the lane
//! that covers <branch>. That build covers an older main, since another
//! landing that changed the lane reached main after it. Land then builds the
//! result with the same steps, test classes and shots before it pushes, so the
//! branch's agent never rebases and builds again because another landing came
//! first. A lane with no build of the branch refuses the landing: the agent
//! runs `builds app-build` on the branch first.
//!
//! The command prints nothing when every lane is covered, and otherwise one
//! line: the `builds app-build <repo> <result> <step>... [--test <lane>
//! <Class>]...` command that builds the result in the lanes no run covers,
//! with the steps and picks of the branch's build of each. A step, class or
//! shots prefix holds no white space (app_build.rs refuses one that does), so
//! land splits the line into its words.
//!
//! The command reads the latest passing runs of each lane
//! (`app_timing::PASSED_RUNS_READ`), newest first, and stops at the first
//! that covers. A run whose commit git does not have in the worktree's
//! repository covers nothing, and the command says so.

use std::path::Path;

use crate::app_config::{CONFIG, Config, PickKind, parse_config, run_git};
use crate::app_timing::PassedRun;
use crate::derive::paths_between;
use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;
use crate::places::Places;

/// The flag of `builds app-build` that names this command.
pub const FLAG: &str = "--landing-build";

const USAGE: &str = "usage: builds app-build --landing-build <worktree> <before> <result> <branch>";

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

/// Whether the change of <paths> changes some lane's apps in <config>: one
/// of the paths matches an `on` line that names a step of the lane.
fn changes_apps(config: &Config, paths: &[String]) -> bool {
    Lane::ALL
        .into_iter()
        .any(|lane| !lane_paths(config, lane, paths).is_empty())
}

/// The `.app-build` of the commit <rev> in the repository at <top>, or None
/// when the commit has none.
fn config_of(top: &Path, rev: &str) -> Result<Option<Config>> {
    if run_git(top, &["ls-tree", "--name-only", rev, "--", CONFIG])?.is_empty() {
        return Ok(None);
    }
    parse_config(&run_git(top, &["show", &format!("{rev}:{CONFIG}")])?).map(Some)
}

/// Whether the commits <shas> in the repository at <top>, a landing's,
/// change what some lane's apps compile, by the `.app-build` of the last
/// one. Commits whose last has no `.app-build` change no apps.
pub fn commits_change_apps(top: &Path, shas: &[String]) -> Result<bool> {
    let last = shas.last().expect("a landing names a commit");
    let Some(config) = config_of(top, last)? else {
        return Ok(false);
    };
    let mut paths = Vec::new();
    for sha in shas {
        paths.extend(paths_between(top, &format!("{sha}^"), sha)?);
    }
    Ok(changes_apps(&config, &paths))
}

/// A lane whose apps the landing changes and that no passing run covers at
/// the result, with no passing run that covers the branch: the newest passing
/// run git has and the paths where its commit differs from the branch, or
/// None when the lane has no such run.
#[derive(Debug, PartialEq, Eq)]
pub struct Unbuilt {
    pub lane: Lane,
    pub newest: Option<(String, Vec<String>)>,
}

/// What a lane whose apps the landing changes and that no passing run covers
/// at the result needs.
#[derive(Debug, PartialEq, Eq)]
pub enum Need {
    /// A build of the result with <steps>, the names of the steps, test runs
    /// and shots runs that the branch's build of the lane passed, as the store
    /// records them.
    Build {
        lane: Lane,
        steps: Vec<String>,
    },
    Unbuilt(Unbuilt),
}

/// Gives the paths where a commit (the first argument) differs from another
/// (the second), or None when git does not have the first.
pub type Differ<'a> = dyn FnMut(&str, &str) -> Result<Option<Vec<String>>> + 'a;

/// How a lane's passing runs stand against one commit.
enum Search {
    Covers(PassedRun),
    /// No run covers it: the newest run git has, with the paths where its
    /// commit differs from the commit at the lane's paths.
    Newest(Option<(String, Vec<String>)>),
}

/// The newest of <runs> that covers <target> in <lane>: it passed every step
/// of <needed>, and its commit has <target>'s files at every path the lane's
/// `on` lines match. <differ> gives the paths where a commit differs from
/// <target>, or None when git does not have the commit.
fn search(
    config: &Config,
    lane: Lane,
    needed: &[String],
    runs: &[PassedRun],
    target: &str,
    differ: &mut Differ,
) -> Result<Search> {
    let mut newest = None;
    for run in runs {
        let Some(differs) = differ(&run.commit, target)? else {
            continue;
        };
        let paths: Vec<String> = lane_paths(config, lane, &differs)
            .into_iter()
            .map(str::to_string)
            .collect();
        if paths.is_empty() && needed.iter().all(|step| run.steps.contains(step)) {
            return Ok(Search::Covers(run.clone()));
        }
        if newest.is_none() {
            newest = Some((run.commit.clone(), paths));
        }
    }
    Ok(Search::Newest(newest))
}

/// What each lane that <changed>, the paths the landing changes, changes in
/// <config> and that no passing run covers at <result> needs, from the
/// lane's passing runs that cover <branch>. <passed> gives a lane's latest
/// passing runs, newest first.
pub fn needs(
    config: &Config,
    changed: &[String],
    result: &str,
    branch: &str,
    passed: &mut dyn FnMut(Lane) -> Result<Vec<PassedRun>>,
    differ: &mut Differ,
) -> Result<Vec<Need>> {
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
        let runs = passed(lane)?;
        if let Search::Covers(_) = search(config, lane, &needed, &runs, result, differ)? {
            continue;
        }
        found.push(
            match search(config, lane, &needed, &runs, branch, differ)? {
                Search::Covers(run) => Need::Build {
                    lane,
                    steps: run.steps,
                },
                Search::Newest(newest) => Need::Unbuilt(Unbuilt { lane, newest }),
            },
        );
    }
    Ok(found)
}

/// The `builds app-build` command that builds <result> of <repo> in each
/// lane of <builds> with the steps, test classes and shots its steps name:
/// the steps first, as the form that names steps takes them, then a flag for
/// each test class and shots run.
pub fn build_command(repo: &str, result: &str, builds: &[(Lane, &[String])]) -> String {
    let mut steps: Vec<&str> = Vec::new();
    let mut picks: Vec<crate::app_build::Pick> = Vec::new();
    for (lane, names) in builds {
        for name in names.iter() {
            match crate::app_build::pick_kind_of(name) {
                None => steps.push(name),
                Some(kind) => {
                    let arg = &name[kind.step_prefix().len()..];
                    match kind {
                        // One run of a test line names all of its classes.
                        PickKind::Class => picks
                            .extend(arg.split(' ').map(|class| (*lane, kind, class.to_string()))),
                        PickKind::Shots => picks.push((*lane, kind, arg.to_string())),
                    }
                }
            }
        }
    }
    let flags: Vec<&str> = picks
        .iter()
        .flat_map(crate::app_build::pick_words)
        .collect();
    let mut words = vec!["builds", "app-build", repo, result];
    words.extend(steps);
    words.extend(flags);
    words.join(" ")
}

/// What land prints when some lanes of the landing of <repo> from <worktree>
/// have no passing build of <branch>.
fn refusal(repo: &str, worktree: &str, branch: &str, lanes: &[&Unbuilt]) -> String {
    let short = |commit: &str| commit[..commit.len().min(12)].to_string();
    let mut said = format!(
        "the result changes what the apps of {repo} compile, and no passing app build built the files of the branch's commit {} for these lanes:",
        short(branch)
    );
    for Unbuilt { lane, newest } in lanes {
        let lane = lane.word();
        match newest {
            None => said.push_str(&format!(
                "\n  {lane}: the store records no passing {lane} app build of a commit git has"
            )),
            Some((commit, paths)) if paths.is_empty() => said.push_str(&format!(
                "\n  {lane}: the newest passing {lane} app build, of {}, did not pass every step the change needs",
                short(commit)
            )),
            Some((commit, paths)) => {
                let shown: Vec<&str> = paths.iter().take(10).map(String::as_str).collect();
                let more = if paths.len() > shown.len() {
                    format!(" and {} more", paths.len() - shown.len())
                } else {
                    String::new()
                };
                said.push_str(&format!(
                    "\n  {lane}: the newest passing {lane} app build, of {}, has other files than the branch at {}{more}",
                    short(commit),
                    shown.join(", ")
                ));
            }
        }
    }
    said.push_str(&format!(
        "\nNothing landed. Next: in {worktree}, run `builds app-build {repo} <branch>` with the branch's test classes and shots, and land again. Land builds the result itself when main moved after that build."
    ));
    said
}

/// `builds app-build --landing-build <worktree> <before> <result> <branch>`:
/// prints the command of the build the landing needs, nothing when it needs
/// none, or the refusal, failing, when some lane has no build of the branch.
/// A repo in no project records no runs, and a result with no `.app-build`
/// builds no apps, so neither needs a build.
pub fn cmd_landing_build(places: &Places, args: &[String]) -> Result<()> {
    let [worktree, before, result, branch] = args else {
        fail!("{USAGE}");
    };
    let top = Path::new(worktree);
    let Some(checkout) = crate::checkout::Checkout::at(top)? else {
        fail!("git finds no checkout at {worktree}");
    };
    let Some(config) = config_of(top, result)? else {
        crate::text::note(format!(
            "{result} has no {CONFIG}, so it builds no apps and needs no app build"
        ));
        return Ok(());
    };
    let repo = checkout.repo();
    let Some(database) = crate::app_timing::database_of(places, &repo)? else {
        crate::text::note(format!(
            "repo {repo} is in no project, so no store records its app builds and its landings need none"
        ));
        return Ok(());
    };
    let changed = paths_between(top, before, result)?;
    let conn = crate::db::open(&database, crate::db::Make::Never)?;
    let found = needs(
        &config,
        &changed,
        result,
        branch,
        &mut |lane| crate::app_timing::passed_runs(&conn, &repo, lane, &database),
        &mut |commit, target| {
            if !crate::review::commits::exists(top, commit)? {
                crate::text::note(format!(
                    "a passing app build built {commit}, which git does not have in {worktree}, so that build covers nothing"
                ));
                return Ok(None);
            }
            paths_between(top, commit, target).map(Some)
        },
    )?;
    let unbuilt: Vec<&Unbuilt> = found
        .iter()
        .filter_map(|need| match need {
            Need::Unbuilt(unbuilt) => Some(unbuilt),
            Need::Build { .. } => None,
        })
        .collect();
    if !unbuilt.is_empty() {
        println!("{}", refusal(&repo, worktree, branch, &unbuilt));
        return Err(crate::fail::Fail::Exit(1));
    }
    let builds: Vec<(Lane, &[String])> = found
        .iter()
        .filter_map(|need| match need {
            Need::Build { lane, steps } => Some((*lane, steps.as_slice())),
            Need::Unbuilt(_) => None,
        })
        .collect();
    if !builds.is_empty() {
        println!("{}", build_command(&repo, result, &builds));
    }
    Ok(())
}

#[cfg(test)]
#[path = "app_landing_tests.rs"]
mod tests;

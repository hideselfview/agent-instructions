//! `builds app-build --landing-coverage <worktree> <branch> <ref>
//! [<own commit>...]`: the check that a landing's branch has the passing app
//! builds it needs. `builds land` runs it after the lists' checks and before
//! it fetches anything, with <branch>, the last commit it lands as the branch
//! holds it, <ref>, the name the branch's app builds were asked to build, and
//! the landing's <own commit>s (<branch> alone when none are named). Land runs
//! no app build of the result and takes no turn: the branch's own passing
//! build is what it trusts, and CI checks `main` as a whole.
//!
//! Each lane's `on` lines in the branch's `.app-build` (app_config.rs) name
//! the paths whose change the lane's apps compile. The landing changes a
//! lane's apps when a path that one of its own commits changes matches one of
//! the lane's `on` lines. A passing run of the lane covers the branch when it
//! passed every step the landing's change needs in the lane, covered every shots
//! prefix the branch's own app builds ran since its merge base, and the commit
//! it built has the branch commit's files at every path the lane's `on` lines
//! match. A run can cover a prefix by passing narrower shots steps that
//! together ran every shot the prefix selects at the branch commit. An agent
//! drops a prefix from that set only by recording why
//! (app_shots_drop.rs), and land prints each drop it honors. Without that
//! rule, an agent could run the shots of the screens a change touches, see
//! one fail as worse than its base, run other shots only, and land on the
//! second, passing run.
//!
//! The command reads the passing runs of each lane that are the branch's
//! (`app_timing::passed_runs`): runs whose ref is <ref>, and runs of a commit
//! the branch holds since its merge base with origin/main. It stops at the
//! newest that covers. A run whose commit git does not have in the worktree's
//! repository covers nothing, and the command says so.
//!
//! The command prints a line `shots dropped: ...` for each drop it honors and
//! a line `shots the base never rendered: ...` for each shots run of a
//! covering build that has such pairs (they fail nothing, and the record shows
//! them). A lane with no covering build makes it print the refusal and fail.
//! A landing that changes no lane's apps, a branch with no `.app-build`, a
//! repo in no project and a branch whose commits are all on origin/main need
//! no build, and the command says so.

use std::collections::HashSet;
use std::path::Path;

use crate::app_config::{CONFIG, Config, config_in, run_git};
use crate::app_shots_drop::Drop;
use crate::app_timing::{BuildRecords, PassedRun};
use crate::app_waiting_shots::Waiting;
use crate::derive::paths_between;
use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;
use crate::places::Places;
use crate::shots_declared::{ShotDeclarations, ShotNames};

/// The flag of `builds app-build` that names this command.
pub const FLAG: &str = "--landing-coverage";

/// What starts each line that tells of a dropped shots prefix.
pub const DROPPED: &str = "shots dropped:";

/// What starts each line that tells of the pairs a shots run of the landing's
/// build rendered that the base never rendered.
pub const UNRENDERED: &str = "shots the base never rendered:";

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

/// The `.app-build` of the commit <rev> in the repository at <top>, or None
/// when the commit has none.
fn config_of(top: &Path, rev: &str) -> Result<Option<Config>> {
    if run_git(top, &["ls-tree", "--name-only", rev, "--", CONFIG])?.is_empty() {
        return Ok(None);
    }
    config_in(top, rev).map(Some)
}

/// The paths that the commits <shas> in the repository at <top> change, each
/// against its parent, without repeats.
fn paths_of_commits(top: &Path, shas: &[String]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for sha in shas {
        paths.extend(paths_between(top, &format!("{sha}^"), sha)?);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// The newest passing run git has of a lane, which covers no commit of a
/// landing: its commit, the paths where that differs from the commit at the
/// lane's `on` lines, and the shots prefixes it did not run that the landing
/// requires.
#[derive(Debug, PartialEq, Eq)]
pub struct Newest {
    pub commit: String,
    pub paths: Vec<String>,
    pub missing_shots: Vec<String>,
}

/// A lane whose apps the landing changes and that no passing run covers: the
/// newest passing run git has, or None when the lane has no such run.
#[derive(Debug, PartialEq, Eq)]
pub struct Unbuilt {
    pub lane: Lane,
    pub newest: Option<Newest>,
}

/// Gives the paths where a commit (the first argument) differs from another
/// (the second), or None when git does not have the first.
pub type Differ<'a> = dyn FnMut(&str, &str) -> Result<Option<Vec<String>>> + 'a;

/// How a lane's passing runs stand against one commit.
enum Search {
    Covers(PassedRun),
    /// No run covers it: the newest run git has.
    Newest(Option<Newest>),
}

/// Gives the names declared in a lane at a commit, when they can be read.
type DeclaredNames<'a> = dyn FnMut(Lane, &str) -> Option<ShotNames> + 'a;

/// A passing run covers a selection directly or through narrower prefixes
/// that together cover every shot the selection names.
pub fn covers_prefix(
    run: &PassedRun,
    prefix: &str,
    (lane, target): (Lane, &str),
    declared: &mut DeclaredNames<'_>,
    waiting: &HashSet<String>,
) -> bool {
    if run.ran_shots(prefix) {
        return true;
    }
    // A scene that waits on app work needs no run: the design record covers
    // it. A prefix that selects only such scenes needs no run at all.
    if !waiting.is_empty()
        && declared(lane, target).is_some_and(|names| {
            let mut selects = names.iter().filter(|name| name.starts_with(prefix));
            selects.clone().next().is_some() && selects.all(|name| waiting.contains(name))
        })
    {
        return true;
    }
    if !run.shots_prefixes().any(|ran| ran.starts_with(prefix)) {
        return false;
    }
    let Some(names) = declared(lane, target) else {
        return false;
    };
    let mut selected = names
        .iter()
        .filter(|name| name.starts_with(prefix) && !waiting.contains(*name))
        .peekable();
    selected.peek().is_some()
        && declared(lane, &run.commit.sha)
            .is_some_and(|built| selected.all(|name| built.contains(name) && run.ran_shots(name)))
}

/// The required prefixes that <run> does not cover at <target>. A narrower
/// prefix can contribute only to shots whose names start with both prefixes.
fn missing_shots(
    run: &PassedRun,
    required: &[String],
    at: (Lane, &str),
    declared: &mut DeclaredNames<'_>,
    waiting: &HashSet<String>,
) -> Vec<String> {
    required
        .iter()
        .filter(|prefix| !covers_prefix(run, prefix, at, declared, waiting))
        .cloned()
        .collect()
}

/// The newest of <runs> that covers <target> in <lane>: it passed every step
/// of <needed>, covered every shots prefix of <required>, and its commit has
/// <target>'s files at every path the lane's `on` lines match. <differ> gives
/// the paths where a commit differs from <target>, or None when git does not
/// have the commit.
fn search(
    config: &Config,
    lane: Lane,
    (needed, required): (&[String], &[String]),
    runs: &[PassedRun],
    target: &str,
    differ: &mut Differ,
    (declared, waiting): (&mut DeclaredNames<'_>, &HashSet<String>),
) -> Result<Search> {
    let mut newest = None;
    for run in runs {
        let Some(differs) = differ(&run.commit.sha, target)? else {
            continue;
        };
        let paths: Vec<String> = lane_paths(config, lane, &differs)
            .into_iter()
            .map(str::to_string)
            .collect();
        if paths.is_empty()
            && needed.iter().all(|step| run.steps.contains(step))
            && missing_shots(run, required, (lane, target), declared, waiting).is_empty()
        {
            return Ok(Search::Covers(run.clone()));
        }
        if newest.is_none() {
            newest = Some((run, paths));
        }
    }
    let newest = newest.map(|(run, paths)| Newest {
        commit: run.commit.sha.clone(),
        paths,
        missing_shots: missing_shots(run, required, (lane, target), declared, waiting),
    });
    Ok(Search::Newest(newest))
}

pub struct Coverage {
    covering: Vec<(Lane, PassedRun)>,
    pub unbuilt: Vec<Unbuilt>,
}

/// Finds the covering build or newest unbuilt candidate for each lane the
/// landing's own commits change. <passed> gives the branch's passing runs in
/// a lane, newest first, and <shots_required> gives the prefixes they must
/// cover: those the branch's app builds ran, less the dropped ones.
pub fn needs(
    config: &Config,
    changed: &[String],
    branch: &str,
    passed: &mut dyn FnMut(Lane) -> Result<Vec<PassedRun>>,
    shots_required: &mut dyn FnMut(Lane) -> Result<Vec<String>>,
    differ: &mut Differ,
    (declared, waiting): (&mut DeclaredNames<'_>, &HashSet<String>),
) -> Result<Coverage> {
    let mut covering = Vec::new();
    let mut unbuilt = Vec::new();
    for lane in config.lanes.iter().copied() {
        if lane_paths(config, lane, changed).is_empty() {
            continue;
        }
        let needed: Vec<String> = crate::derive::plan(config, changed, &[])
            .into_iter()
            .filter(|(own, _)| *own == lane)
            .flat_map(|(_, steps)| steps)
            .collect();
        let runs = passed(lane)?;
        let required = shots_required(lane)?;
        let asked = (needed.as_slice(), required.as_slice());
        match search(
            config,
            lane,
            asked,
            &runs,
            branch,
            differ,
            (&mut *declared, waiting),
        )? {
            Search::Covers(run) => covering.push((lane, run)),
            Search::Newest(newest) => unbuilt.push(Unbuilt { lane, newest }),
        }
    }
    Ok(Coverage { covering, unbuilt })
}

/// What land prints when some lanes of the landing of <repo> from <worktree>
/// have no passing build of <branch>.
fn refusal(repo: &str, worktree: &str, branch: &str, lanes: &[&Unbuilt]) -> String {
    let short = |commit: &str| commit[..commit.len().min(12)].to_string();
    let mut said = format!(
        "the landing changes what the apps of {repo} compile, and no passing app build built the files of the branch's commit {} for these lanes:",
        short(branch)
    );
    for Unbuilt { lane, newest } in lanes {
        let lane = lane.word();
        let Some(Newest {
            commit,
            paths,
            missing_shots,
        }) = newest
        else {
            said.push_str(&format!(
                "\n  {lane}: the store records no passing {lane} app build of a commit git has"
            ));
            continue;
        };
        if paths.is_empty() && missing_shots.is_empty() {
            said.push_str(&format!(
                "\n  {lane}: the newest passing {lane} app build, of {}, did not pass every step the change needs",
                short(commit)
            ));
        }
        if !paths.is_empty() {
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
        if !missing_shots.is_empty() {
            said.push_str(&format!(
                "\n  {lane}: the newest passing {lane} app build, of {}, did not cover the shots prefixes {} that the app builds of the branch ran since its merge base",
                short(commit),
                missing_shots.join(", ")
            ));
        }
    }
    if lanes.iter().any(|unbuilt| {
        unbuilt
            .newest
            .as_ref()
            .is_some_and(|n| !n.missing_shots.is_empty())
    }) {
        said.push_str(&format!(
            "\nA passing app build must cover every shots prefix the branch's builds ran. To drop a prefix, record why with `builds app-build {FLAG_DROP} {repo} <branch> <lane> <prefix> --why TEXT`. Land prints the reason.",
            FLAG_DROP = crate::app_shots_drop::FLAG
        ));
    }
    said.push_str(&format!(
        "\nNothing landed. Next: in {worktree}, run `builds app-build {repo} <branch>` with the branch's test classes and shots, and land again. Land builds nothing itself."
    ));
    said
}

/// When the first commit of <branch> that <before> lacks was written, in
/// seconds since 1970: the author date a rebase or an amend keeps. App builds
/// of the branch started after it.
pub fn branch_since(top: &Path, before: &str, branch: &str) -> Result<i64> {
    let range = format!("{before}..{branch}");
    let dates = run_git(top, &["log", "--reverse", "--format=%at", &range])?;
    let Some(first) = dates.lines().next() else {
        fail!("{branch} has no commit that {before} lacks");
    };
    match first.trim().parse() {
        Ok(at) => Ok(at),
        Err(_) => fail!("git printed {first:?}, not a time, for the first commit of {range}"),
    }
}

/// What land prints for each of <drops>, the prefixes dropped from the
/// landing of <branch> in <lane>.
fn dropped_lines(lane: Lane, drops: &[Drop]) -> Vec<String> {
    drops
        .iter()
        .map(|Drop { prefix, by, why }| {
            format!(
                "{DROPPED} the {} shots prefix {prefix} needs no run before the landing; {by} recorded why: {why}",
                lane.word()
            )
        })
        .collect()
}

/// What land prints for the shots run of <prefix> in <lane> that the build of
/// <commit> made in <environment>, when the run's output <findings> has pairs
/// the base never rendered; None when it has none.
fn unrendered_line(
    lane: Lane,
    commit: &str,
    environment: &str,
    prefix: &str,
    findings: &str,
) -> Option<String> {
    let said = findings
        .lines()
        .find(|line| crate::shot_findings::says_unrendered(line))?;
    Some(format!(
        "{UNRENDERED} the {} shots run {prefix} of {}, made in environment {environment}: {}",
        lane.word(),
        &commit[..commit.len().min(12)],
        said.trim()
    ))
}

/// The lines that tell of the pairs the base never rendered in the shots runs
/// of <run>, a passing build of <lane> that covers the landing of <repo>.
fn unrendered_lines(
    places: &Places,
    repo: &str,
    lane: Lane,
    run: &PassedRun,
) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for prefix in run.shots_prefixes() {
        let found = crate::shots_cache::covering_run(
            places,
            repo,
            &run.commit.sha,
            lane,
            crate::shots_cache::Made::Anywhere,
            prefix,
            &crate::shots_program::VERSIONS,
        )?;
        match found {
            Some(cached) => lines.extend(unrendered_line(
                lane,
                &run.commit.sha,
                &cached.key.environment,
                prefix,
                &cached.findings,
            )),
            None => lines.push(format!(
                "{UNRENDERED} the {} shots run {prefix} of {} is no longer cached, so land cannot say whether the base rendered its pairs",
                lane.word(),
                &run.commit.sha[..run.commit.sha.len().min(12)]
            )),
        }
    }
    Ok(lines)
}

/// The branch whose own app builds can cover its changed paths.
pub struct BranchBuilds<'a> {
    top: &'a Path,
    target: &'a str,
    named: &'a str,
    commits: Vec<String>,
    since: i64,
}

type DroppedShots = Vec<(Lane, Vec<Drop>)>;

/// What a coverage check found beside the covering builds: the prefixes the
/// landing drops, and the scenes that wait on app work, which it counts
/// covered.
pub struct Excused {
    pub dropped: DroppedShots,
    pub waiting: Vec<Waiting>,
}

pub enum ShotsToCover<'a> {
    Landing,
    Build(&'a crate::app_build::BuildPicks),
}

impl<'a> BranchBuilds<'a> {
    pub fn off_main(top: &'a Path, target: &'a str, named: &'a str) -> Result<Option<Self>> {
        let commits = crate::git::commits_in_range(top, "origin/main", target)?;
        if commits.is_empty() {
            return Ok(None);
        }
        let since = branch_since(top, "origin/main", target)?;
        Ok(Some(Self {
            top,
            target,
            named,
            commits,
            since,
        }))
    }

    /// Finds covering builds for a landing's shot requirements or a build's
    /// requested comparisons. Only a landing applies recorded drops.
    pub fn coverage(
        &self,
        config: &Config,
        changed: &[String],
        records: &BuildRecords<'_>,
        shots: ShotsToCover<'_>,
        declarations: &mut ShotDeclarations<'_>,
    ) -> Result<(Coverage, Excused)> {
        let top = self.top;
        let drops: DroppedShots = match shots {
            ShotsToCover::Landing => config
                .lanes
                .iter()
                .copied()
                .map(|lane| Ok((lane, records.dropped(self.named, lane, self.since)?)))
                .collect::<Result<_>>()?,
            ShotsToCover::Build(_) => Vec::new(),
        };
        // A landing counts the scenes that wait on app work covered; a build's
        // requested comparisons name the shots it ran, which leave them out
        // already.
        let waiting = match shots {
            ShotsToCover::Landing => {
                crate::app_waiting_shots::waiting_in(&records.lists()?, records.repo, changed)
            }
            ShotsToCover::Build(_) => Vec::new(),
        };
        let scenes = crate::app_waiting_shots::scenes(&waiting);
        let coverage = needs(
            config,
            changed,
            self.target,
            &mut |lane| records.passed_runs(lane, Some(self.named), &self.commits),
            &mut |lane| {
                if let ShotsToCover::Build(picks) = shots {
                    return Ok(picks.reused_in(lane).map(str::to_string).collect());
                }
                let ran = records.shots_ran(lane, self.named, &self.commits, self.since)?;
                let drops = drops
                    .iter()
                    .flat_map(|(own, drops)| drops.iter().filter(move |_| *own == lane));
                let dropped: HashSet<&str> = drops.map(|drop| drop.prefix.as_str()).collect();
                Ok(ran
                    .into_iter()
                    .filter(|prefix| !dropped.contains(prefix.as_str()))
                    .collect())
            },
            &mut |commit, target| {
                if !crate::git::commit_exists(top, commit)? {
                    crate::output::note(format!(
                        "a passing app build built {commit}, which git does not have in {}, so that build covers nothing",
                        top.display()
                    ));
                    return Ok(None);
                }
                paths_between(top, commit, target).map(Some)
            },
            (
                &mut |lane, commit| declarations.names(lane, commit),
                &scenes,
            ),
        )?;
        Ok((
            coverage,
            Excused {
                dropped: drops,
                waiting,
            },
        ))
    }
}

/// `builds app-build --landing-coverage <worktree> <branch> <ref>
/// [<own commit>...]`: prints a line for each dropped shots prefix it honors
/// and for each covering run with pairs the base never rendered, or the
/// refusal, failing, when some lane the landing changes has no covering build
/// of the branch. A repo in no project records no runs, and a branch with no
/// `.app-build` builds no apps, so neither needs a build.
pub fn cmd_landing_coverage(places: &Places, args: &crate::args::Args) -> Result<()> {
    let [worktree, branch, named, own @ ..] = args.positional() else {
        fail!("{}", args.command().usage_text());
    };
    let own = if own.is_empty() {
        std::slice::from_ref(branch)
    } else {
        own
    };
    let top = Path::new(worktree);
    let Some(checkout) = crate::checkout::Checkout::at(top)? else {
        fail!("git finds no checkout at {worktree}");
    };
    let Some(config) = config_of(top, branch)? else {
        crate::output::note(format!(
            "{branch} has no {CONFIG}, so it builds no apps and needs no app build"
        ));
        return Ok(());
    };
    let repo = checkout.repo();
    let Some(records) = BuildRecords::open(places, &repo)? else {
        crate::output::note(format!(
            "repo {repo} is in no project, so no store records its app builds and its landings need none"
        ));
        return Ok(());
    };
    let Some(branch_builds) = BranchBuilds::off_main(top, branch, named)? else {
        crate::output::note(format!(
            "every commit of {branch} is on origin/main, so it needs no app build"
        ));
        return Ok(());
    };
    let changed = paths_of_commits(top, own)?;
    let (Coverage { covering, unbuilt }, Excused { dropped, waiting }) = branch_builds.coverage(
        &config,
        &changed,
        &records,
        ShotsToCover::Landing,
        &mut ShotDeclarations::new(top),
    )?;
    for (lane, drops) in dropped {
        for line in dropped_lines(lane, &drops) {
            crate::output::say(line);
        }
        for line in crate::app_waiting_shots::landing_lines(&waiting, lane) {
            crate::output::say(line);
        }
    }
    for (lane, run) in &covering {
        for line in unrendered_lines(places, &repo, *lane, run)? {
            crate::output::say(line);
        }
    }
    if !unbuilt.is_empty() {
        let unbuilt: Vec<&Unbuilt> = unbuilt.iter().collect();
        crate::output::answer_line(refusal(&repo, worktree, branch, &unbuilt));
        return Err(crate::fail::Fail::Exit(1));
    }
    Ok(())
}

#[cfg(test)]
#[path = "app_landing_tests.rs"]
mod tests;

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
//! with (`mac.welcome.`), and app-build refuses a prefix that selects every
//! shot of the lane (`mac.`). `--all-shots <lane>` runs that whole set in the
//! lane. A lane with such a run builds
//! what its changed paths need, or, when they need nothing in that lane, what
//! its `needs` line names, so the run has a build of <ref> to run against. A
//! `--shots` or `--all-shots` run whose findings for <ref> are already cached
//! prints them instead of running, unless `--fresh`, which also builds the
//! app and takes the shots again, replacing their pictures in the folder.
//!
//! The lane's process (app_build.rs) then picks the checkout: the free one on
//! the less loaded host, `here`-only steps staying on this Mac (lock.rs).

use std::collections::HashSet;
use std::path::Path;

use crate::app_build::{self, Pick};
use crate::app_config::{CONFIG, Config, PickKind, run_git};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::text::note;
use boards::git;

const USAGE: &str = "usage: builds app-build <repo> <ref> [--test <Class>]... [--shots <prefix>]... [--all-shots mac|android]... [--fresh]";

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

/// The full id of <commit>'s merge-base with `origin/main` in the repository
/// at <top>.
pub fn merge_base(top: &Path, commit: &str) -> Result<String> {
    Ok(run_git(top, &["merge-base", commit, "origin/main"])?
        .trim()
        .to_string())
}

/// The paths that differ between <commit> and its merge-base with
/// `origin/main`, relative to the repo's top.
pub fn changed_paths(top: &Path, commit: &str) -> Result<Vec<String>> {
    paths_between(top, &merge_base(top, commit)?, commit)
}

/// The paths that differ between the commits <from> and <to> in the
/// repository at <top>, relative to its top.
pub fn paths_between(top: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    let diff = run_git(top, &["diff", "--name-only", from, to])?;
    Ok(diff.lines().map(str::to_string).collect())
}

/// The keywords that declare a test type: Swift's `class`, `struct`, `enum` and
/// `actor` (Swift Testing suites are structs), Kotlin's `class` and `object`
/// (with any modifiers before them).
const DECLARING_WORDS: &str = "class|struct|enum|actor|object";

/// The extended regular expression that matches a line declaring the type
/// <class>: a declaring keyword, spaces, the whole name.
fn declaration_pattern(class: &str) -> String {
    format!("(^|[^A-Za-z0-9_])({DECLARING_WORDS})[[:space:]]+{class}([^A-Za-z0-9_]|$)")
}

/// Does a file in <folders> at <commit> declare the type <class>?
fn declares(top: &Path, commit: &str, class: &str, folders: &[&str]) -> Result<bool> {
    let output = git(top)
        .args([
            "grep",
            "-l",
            "-E",
            "-e",
            &declaration_pattern(class),
            commit,
            "--",
        ])
        .args(folders)
        .output()
        .context(|| "running git to search the sources".to_string())?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => fail!(
            "git failed to search the sources: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

/// <lane>'s `sources` folders for <program> (None: the lane's default).
fn folders_of<'a>(config: &'a Config, lane: Lane, program: Option<&str>) -> Vec<&'a str> {
    config
        .sources
        .iter()
        .filter(|source| source.lane == lane && source.program.as_deref() == program)
        .map(|source| source.folder.as_str())
        .collect()
}

/// The lanes whose `sources` folders hold a file at <commit> that declares
/// the type <class> (a plain mention of the name does not count).
fn lanes_declaring(top: &Path, config: &Config, commit: &str, class: &str) -> Result<Vec<Lane>> {
    let mut found = Vec::new();
    for lane in Lane::ALL {
        let folders: Vec<&str> = config
            .sources
            .iter()
            .filter(|source| source.lane == lane)
            .map(|source| source.folder.as_str())
            .collect();
        if !folders.is_empty() && declares(top, commit, class, &folders)? {
            found.push(lane);
        }
    }
    Ok(found)
}

/// The program that runs the test class <class> in <lane>: the named program
/// (a `test <lane>:<program>` line) whose `sources` folders declare it at
/// <commit>, else None, the lane's default. The class's folder decides, so
/// `--test` needs no word for the program.
pub fn program_of(
    top: &Path,
    config: &Config,
    commit: &str,
    lane: Lane,
    class: &str,
) -> Result<Option<String>> {
    let mut programs: Vec<&str> = config
        .sources
        .iter()
        .filter(|source| source.lane == lane)
        .filter_map(|source| source.program.as_deref())
        .collect();
    programs.dedup();
    for program in programs {
        let folders = folders_of(config, lane, Some(program));
        if declares(top, commit, class, &folders)? {
            return Ok(Some(program.to_string()));
        }
    }
    Ok(None)
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
/// in (an `--all-shots <lane>` names its lane), and whether `--fresh` was
/// given.
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
        if flag == app_build::ALL_SHOTS {
            let (pick, after) = app_build::all_shots_flag(after, USAGE)?;
            picks.push(pick);
            rest = after;
            continue;
        }
        let Some(kind) = PickKind::ALL.into_iter().find(|kind| flag == kind.flag()) else {
            fail!(
                "{flag:?} where --test, --shots, {} or --fresh goes\n{USAGE}",
                app_build::ALL_SHOTS
            );
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
        if kind == PickKind::Class && !arg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            fail!(
                "--test takes one test class's type name (letters, digits, underscores), not {arg:?}: {}",
                app_build::ONLY_WHAT_A_CHANGE_TOUCHES
            );
        }
        let lane = lane_of(kind, arg)?;
        if kind == PickKind::Shots {
            app_build::refuse_whole_lane(lane, arg)?;
        }
        picks.push((lane, kind, arg.clone()));
        rest = after;
    }
    Ok((picks, fresh))
}

/// A branch cut before the repo declared the `on`, `needs` and `sources` lines
/// has none in its `.app-build`, and so no way to choose steps or find a
/// class's lane: it takes those lines from `origin/main`'s `.app-build`. A
/// branch with any of the three keeps its own.
fn adopt_main_rules(top: &Path, config: &mut Config) -> Result<()> {
    if !(config.rules.is_empty() && config.needs.is_empty() && config.sources.is_empty()) {
        return Ok(());
    }
    let Ok(text) = run_git(top, &["show", &format!("origin/main:{CONFIG}")]) else {
        return Ok(());
    };
    let main = crate::app_config::parse_config(&text)?;
    if main.rules.is_empty() && main.needs.is_empty() && main.sources.is_empty() {
        return Ok(());
    }
    note(format!(
        "this branch's {CONFIG} has no on, needs or sources lines; using origin/main's"
    ));
    config.rules = main.rules;
    config.needs = main.needs;
    config.sources = main.sources;
    Ok(())
}

pub fn derive_and_run(places: &Places, args: &[String]) -> Result<()> {
    let [repo, git_ref, tail @ ..] = args else {
        fail!("{USAGE}");
    };
    if git_ref.starts_with('-') {
        fail!("{USAGE}");
    }
    let (top, commit, mut config) = app_build::commit_and_config(places, repo, git_ref)?;
    adopt_main_rules(&top, &mut config)?;
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
        let program = crate::this_program()?;
        picks = app_build::serve_cached(places, repo, &commit, picks, &program)?;
        if picks.is_empty() {
            return Ok(());
        }
    }
    let pinned = app_build::pin(places)?;
    app_build::ensure_before_runs(places, &pinned, repo, &commit, &picks)?;
    let changed = changed_paths(&top, &commit)?;
    let lanes = plan(&config, &changed, &picks);
    if lanes.is_empty() {
        note(format!(
            "nothing to build: no path {git_ref} changes needs a step ({CONFIG}'s `on` lines)"
        ));
        return Ok(());
    }
    // --fresh takes the shots again, so no lane may skip its build for them.
    let kept = if fresh {
        Vec::new()
    } else {
        app_build::cached_shots_lanes(
            places, repo, &top, &commit, &config, &changed, &lanes, &picks,
        )?
    };
    let lanes: Vec<(Lane, Vec<String>)> = lanes
        .into_iter()
        .map(|(lane, names)| {
            if kept.contains(&lane) {
                (lane, Vec::new())
            } else {
                (lane, names)
            }
        })
        .collect();
    for (lane, names) in &lanes {
        note(format!(
            "{} lane: {}",
            lane.word(),
            if kept.contains(lane) {
                "no build: the picture folder holds every shot the runs take for the lane's app tree".to_string()
            } else if names.is_empty() {
                "no build, only the runs asked for".to_string()
            } else {
                names.join(" ")
            }
        ));
    }
    app_build::run_lanes(&pinned, repo, &commit, &lanes, &picks, &kept, fresh, false)
}

#[cfg(test)]
#[path = "derive_tests.rs"]
mod tests;

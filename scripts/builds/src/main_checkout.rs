//! A repo's main checkout (the folder that holds the common .git, such as
//! ~/dev/bae) against origin/main. The main checkout is the person's, and
//! agents and the coordinator read code there, so `builds land` fast-forwards
//! its main to origin/main after each push. Agents still never edit, build or
//! commit there: land moves main only when git can fast-forward it without
//! touching the person's commits or uncommitted edits, and otherwise leaves
//! the checkout as it is and prints the command to run. In agent-instructions
//! the fast-forward also installs the landed change: the checkout's post-merge
//! hook runs install.sh. `builds health` lists each project's main checkout whose
//! main lacks commits that origin/main has, with the command that brings it
//! forward.
//!
//! This module reads the checkout's refs as they are and fetches nothing.

use std::path::Path;

use crate::health::{Advice, Finding, FindingKind};
use crate::side_git::run;

/// Where a main checkout stands against origin/main.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Standing {
    /// The branch checked out, or None when HEAD is detached.
    pub on: Option<String>,
    /// The commits on origin/main that the checkout's main lacks.
    pub behind: usize,
    /// The commits on the checkout's main that origin/main lacks.
    pub ahead: usize,
}

impl Standing {
    /// Reads the standing of the main checkout at <main>. Errs with what git
    /// said when it cannot, such as when the checkout has no main branch or
    /// no origin/main.
    pub fn read(main: &Path) -> Result<Standing, String> {
        let on = run(main, &["branch", "--show-current"])?.trim().to_string();
        let counts = run(
            main,
            &[
                "rev-list",
                "--left-right",
                "--count",
                "refs/heads/main...refs/remotes/origin/main",
            ],
        )?;
        let numbers: Vec<usize> = counts
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| format!("git rev-list --count printed {counts:?}"))?;
        let [ahead, behind] = numbers[..] else {
            return Err(format!("git rev-list --count printed {counts:?}"));
        };
        Ok(Standing {
            on: (!on.is_empty()).then_some(on),
            behind,
            ahead,
        })
    }

    /// What stops git from fast-forwarding the checkout's main where it is
    /// now, as a clause about the checkout, and what the person does to clear
    /// it. None when the checkout is on main and its main has no commit that
    /// origin/main lacks.
    pub fn obstacle(&self) -> Option<(String, &'static str)> {
        if let Some(why) = off_main(self.on.as_deref()) {
            return Some((why, "checks out main there"));
        }
        (self.ahead > 0).then(|| {
            (
                format!(
                    "has {} on main that origin/main lacks",
                    crate::text::counted(self.ahead, "commit", "commits")
                ),
                "pushes or removes those commits",
            )
        })
    }
}

/// Why a main checkout with <on> checked out (None for a detached HEAD) is
/// not on main, as a clause about the checkout; None when it is on main.
/// A merge there would move that branch, not main.
pub fn off_main(on: Option<&str>) -> Option<String> {
    match on {
        Some("main") => None,
        Some(branch) => Some(format!("is on branch {branch}, not main")),
        None => Some("is on a detached HEAD, not main".to_string()),
    }
}

/// The command that fast-forwards the main checkout at <main>, for the
/// person to run.
pub fn forward_command(main: &Path) -> String {
    format!(
        "git -C {} merge --ff-only origin/main",
        crate::text::shell_word(&main.display().to_string())
    )
}

/// An install that failed, as install.sh records it.
#[derive(Debug, PartialEq, Eq)]
pub struct FailedInstall {
    pub commit: String,
    /// The step install.sh was running when it stopped.
    pub step: String,
}

/// The failed install that the file `builds-install` of the git folder of the
/// main checkout at <main> records. install.sh writes the file (`ok COMMIT`
/// or `failed COMMIT STEP`), since an install that fails before it builds
/// `builds` has no program to write it. None when the file is missing or
/// records a successful install; Err with what git said or what the file
/// holds when that cannot be read.
pub fn failed_install(main: &Path) -> Result<Option<FailedInstall>, String> {
    let dir = run(main, &["rev-parse", "--git-common-dir"])?;
    let file = main.join(dir.trim()).join("builds-install");
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("reading {}: {error}", file.display())),
    };
    let line = text.trim();
    let malformed = || {
        format!(
            "{} holds {line:?}, which is neither `ok COMMIT` nor `failed COMMIT STEP`",
            file.display()
        )
    };
    match line.split_once(' ') {
        Some(("ok", _)) => Ok(None),
        Some(("failed", rest)) => {
            let (commit, step) = rest.split_once(' ').ok_or_else(malformed)?;
            Ok(Some(FailedInstall {
                commit: commit.to_string(),
                step: step.to_string(),
            }))
        }
        _ => Err(malformed()),
    }
}

/// What the failed install of <main> says: the commit, the step, and that
/// the builds on PATH is older than origin/main until an install succeeds.
/// None when the last install did not fail.
pub fn failed_install_words(main: &Path) -> Option<String> {
    match failed_install(main) {
        Ok(Some(FailedInstall { commit, step })) => Some(format!(
            "the install of {} in {} failed at {step}, so the builds on PATH is older than origin/main until an install succeeds",
            &commit[..commit.len().min(8)],
            main.display()
        )),
        Ok(None) => None,
        Err(error) => Some(format!(
            "builds could not read how the last install of {} ended: {error}",
            main.display()
        )),
    }
}

/// The command that installs the main checkout at <main> again, once the
/// cause of the failure is fixed.
pub fn install_command(main: &Path) -> String {
    crate::text::shell_word(&format!("{}/install.sh", main.display()))
}

/// The main checkout (`<home>/dev/<repo>`) of each repo of <repos> that has
/// one on this Mac, each once, in repo order.
pub fn main_checkouts<'a>(
    home: &Path,
    repos: impl Iterator<Item = &'a str>,
) -> Vec<std::path::PathBuf> {
    let repos: std::collections::BTreeSet<&str> = repos.collect();
    repos
        .into_iter()
        .map(|repo| home.join(crate::host::clone_path(repo)))
        .filter(|main| main.is_dir())
        .collect()
}

/// The finding for each main checkout of a project's repo on this Mac
/// (`<home>/dev/<repo>`) whose main lacks commits that origin/main has, or
/// that git could not read. A repo two projects hold counts once, and a repo
/// with no main checkout here is no finding.
pub fn findings(
    home: &Path,
    loaded: &std::result::Result<Vec<crate::lists::store::Loaded>, String>,
) -> Vec<Finding> {
    let Ok(loaded) = loaded else {
        return Vec::new();
    };
    let repos = loaded
        .iter()
        .filter_map(|store| store.lists.as_ref().ok())
        .flat_map(|lists| lists.repos.iter().map(String::as_str));
    main_checkouts(home, repos)
        .into_iter()
        .flat_map(|main| {
            let install = failed_install_words(&main).map(|words| {
                Advice::run(
                    format!("install: {words}; once the cause is fixed, run"),
                    install_command(&main),
                )
            });
            finding(&main).into_iter().chain(install)
        })
        .map(|advice| Finding::new(FindingKind::MainCheckout, advice))
        .collect()
}

/// What health says about the main checkout at <main>, or None when its main
/// has every commit that origin/main has.
fn finding(main: &Path) -> Option<Advice> {
    let standing = match Standing::read(main) {
        Ok(standing) => standing,
        Err(error) => {
            return Some(Advice::said(format!(
                "checkout: builds could not read the main checkout {}: {error}",
                main.display()
            )));
        }
    };
    if standing.behind == 0 {
        return None;
    }
    let lacks = format!(
        "checkout: the main checkout {} lacks {} that origin/main has",
        main.display(),
        crate::text::counted(standing.behind, "commit", "commits")
    );
    let command = forward_command(main);
    Some(match standing.obstacle() {
        None => Advice::run(format!("{lacks}; bring it forward with"), command),
        Some((why, remedy)) => Advice::run(
            format!(
                "{lacks} and {why}, so git cannot fast-forward it; once the person {remedy}, run"
            ),
            command,
        ),
    })
}

#[cfg(test)]
#[path = "main_checkout_tests.rs"]
mod tests;

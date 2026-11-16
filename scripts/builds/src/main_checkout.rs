//! A repo's main checkout (the folder that holds the common .git, such as
//! ~/dev/bae) against origin/main. The main checkout is the person's, and
//! agents and the coordinator read code there, so `builds land` fast-forwards
//! its main to origin/main after each push. Agents still never edit, build or
//! commit there: land moves main only when git can fast-forward it without
//! touching the person's commits or uncommitted edits, and otherwise leaves
//! the checkout as it is and prints the command to run. In agent-instructions
//! the fast-forward also makes the landing live: the checkout's post-merge hook
//! runs install.sh. `builds health` lists each project's main checkout whose
//! main lacks commits that origin/main has, with the command that brings it
//! forward.
//!
//! This module reads the checkout's refs as they are and fetches nothing.

use std::path::Path;
use std::process::Stdio;

use crate::checkout::git;
use crate::health::{Advice, Finding, FindingKind};

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
        let on = read_git(main, &["branch", "--show-current"])?;
        let counts = read_git(
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
        match &self.on {
            Some(branch) if branch == "main" => {}
            Some(branch) => {
                return Some((
                    format!("is on branch {branch}, not main"),
                    "checks out main there",
                ));
            }
            None => {
                return Some((
                    "is on a detached HEAD, not main".to_string(),
                    "checks out main there",
                ));
            }
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

/// The command that fast-forwards the main checkout at <main>, for the
/// person to run.
pub fn forward_command(main: &Path) -> String {
    format!(
        "git -C {} merge --ff-only origin/main",
        crate::text::shell_word(&main.display().to_string())
    )
}

/// What git printed, trimmed, or what it said when it failed. Reads with
/// git's optional locks off, so a read never blocks the person's git.
fn read_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = git(dir)
        .arg("--no-optional-locks")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("running git in {}: {error}", dir.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git -C {} {} failed: {}",
            dir.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
    let repos: std::collections::BTreeSet<&str> = loaded
        .iter()
        .filter_map(|store| store.lists.as_ref().ok())
        .flat_map(|lists| lists.repos.iter().map(String::as_str))
        .collect();
    repos
        .into_iter()
        .map(|repo| home.join(crate::host::clone_path(repo)))
        .filter(|main| main.is_dir())
        .filter_map(|main| finding(&main))
        .map(|advice| Finding::new(FindingKind::MainCheckout, advice))
        .collect()
}

/// What health says about the main checkout at <main>, or None when its main
/// holds origin/main.
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

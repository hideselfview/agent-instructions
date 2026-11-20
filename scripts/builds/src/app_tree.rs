//! A lane's app source tree: what the app a lane builds is made from at a
//! commit, so that the shots the app takes are kept by it (shot_store.rs's
//! picture folder) and a commit that changes none of it takes none again.
//!
//! The paths the lane's app build reads are those the `.app-build` `on`
//! lines name for the lane's steps. The app tree is a hash of the git object
//! of each such path at the commit (a folder's tree, a file's blob, or each
//! file a `*<ending>` pattern matches) and of the `.app-build` itself, which
//! holds the capture commands.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::app_config::{CONFIG, Config, run_git};
use crate::fail::Result;
use crate::lane::Lane;

/// The app source tree of <lane> at <commit> in the repository at <top>,
/// whose `.app-build` at that commit is <config>: 16 hex digits.
pub fn of(top: &Path, commit: &str, config: &Config, lane: Lane) -> Result<String> {
    let lane_steps: Vec<&str> = config
        .steps
        .iter()
        .filter(|step| step.lane == lane)
        .map(|step| step.name.as_str())
        .collect();
    let mut patterns: Vec<&str> = config
        .rules
        .iter()
        .filter(|rule| {
            rule.steps
                .iter()
                .any(|step| lane_steps.contains(&step.as_str()))
        })
        .map(|rule| rule.pattern.as_str())
        .collect();
    patterns.push(CONFIG);
    patterns.sort();
    patterns.dedup();
    let mut hasher = Sha256::new();
    let mut listing: Option<String> = None;
    for pattern in patterns {
        let objects = match pattern.strip_prefix('*') {
            Some(ending) => {
                let files = match &listing {
                    Some(files) => files,
                    None => listing.insert(run_git(top, &["ls-tree", "-r", commit])?),
                };
                files
                    .lines()
                    .filter(|line| line.ends_with(ending))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            None => {
                let path = pattern.trim_end_matches('/');
                run_git(top, &["ls-tree", commit, "--", path])?
            }
        };
        hasher.update(pattern.as_bytes());
        hasher.update([0]);
        hasher.update(objects.trim().as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    Ok(digest[..8].iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
#[path = "app_tree_tests.rs"]
mod tests;

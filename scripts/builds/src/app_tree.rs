//! A lane's app source tree: what the app a lane builds is made from at a
//! commit, so that the shots the app takes are kept by it (shot_store.rs's
//! picture folder) and a commit that changes none of it takes none again.
//!
//! The paths the lane's app build reads are those the `.app-build` `on`
//! lines name for the lane's steps (`Rule::matches`). The app tree is a hash
//! of the git object of each file at the commit that such a line matches,
//! with its path, and of the `.app-build` itself, which holds the capture
//! commands.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::app_config::{CONFIG, Config, Rule, run_git};
use crate::fail;
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
    let rules: Vec<&Rule> = config
        .rules
        .iter()
        .filter(|rule| {
            rule.steps
                .iter()
                .any(|step| lane_steps.contains(&step.as_str()))
        })
        .collect();
    // Each line is `<mode> <type> <object>\t<path>`, sorted by path.
    let listing = run_git(top, &["ls-tree", "-r", "--full-tree", commit])?;
    let mut hasher = Sha256::new();
    for line in listing.lines() {
        let Some((_, path)) = line.split_once('\t') else {
            fail!("git ls-tree printed {line:?}, which names no path after a tab");
        };
        if path == CONFIG || rules.iter().any(|rule| rule.matches(path)) {
            hasher.update(line.as_bytes());
            hasher.update([b'\n']);
        }
    }
    Ok(crate::text::hex(&hasher.finalize()[..8]))
}

#[cfg(test)]
#[path = "app_tree_tests.rs"]
mod tests;

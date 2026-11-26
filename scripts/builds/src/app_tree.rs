//! A lane's app source tree: what the app a lane builds is made from at a
//! commit, so that the shots the app takes are cached by it (shots_cache.rs's
//! picture folder) and a commit that changes none of it takes none again.
//!
//! The picture folder's key for a shot is this tree combined with the
//! environment the shot is taken in (`in_environment`).
//!
//! The paths the lane's app build reads are those the `.app-build` `on`
//! lines name for the lane's steps (`Rule::matches`). The app tree is a hash
//! of the git object of each file at the commit that such a line matches,
//! with its path, and of the `.app-build` itself, which holds the capture
//! commands.
//!
//! The shots program draws each component scene at the size of its board,
//! which it measures from the design system's preview pages and the canvas
//! runtime when it captures, and no `on` line names those files. The app tree
//! holds them too (`scene_size_inputs`), so a change to what sizes a scene
//! keys a new picture folder and no earlier capture of another size is reused.
//!
//! The app also builds from Rust crates, which no `on` line names unless the
//! crate's folder is one (a bridge crate's is). The lane's Rust seeds are the
//! crates whose folders a rule of the lane matches, and the crates a script
//! the rules match builds with `cargo ... -p <crate>` (a generator of the
//! theme or the strings). The app tree holds every file of each seed and of
//! every crate in their dependency closure, which the workspace's manifests
//! at the commit give, and the files every crate builds from: the workspace
//! manifest, `Cargo.lock`, the toolchain pins and `.cargo/`. The closure
//! comes from the manifests in the commit's tree, not from `cargo metadata`,
//! which needs a checkout of the commit.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use boards::Source;
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
    let mut paths = Vec::new();
    for line in listing.lines() {
        let Some((_, path)) = line.split_once('\t') else {
            fail!("git ls-tree printed {line:?}, which names no path after a tab");
        };
        paths.push(path);
    }
    let matched = |path: &str| path == CONFIG || rules.iter().any(|rule| rule.matches(path));
    let crate_dirs = closure_dirs(top, commit, &paths, &matched)?;
    let sizing = scene_size_inputs(top, commit, &paths)?;
    let mut hasher = Sha256::new();
    for (line, path) in listing.lines().zip(&paths) {
        if matched(path)
            || is_rust_input(path, &crate_dirs)
            || sizing.iter().any(|input| is_at(path, input))
        {
            hasher.update(line.as_bytes());
            hasher.update([b'\n']);
        }
    }
    Ok(crate::text::hex(&hasher.finalize()[..8]))
}

/// The paths of the files that size a component scene at <commit>: the design
/// system's files, with what its symbolic links point at, and the record of
/// the canvas runtime that draws the boards. Empty when <paths>, the commit's
/// files, holds no design system.
fn scene_size_inputs(top: &Path, commit: &str, paths: &[&str]) -> Result<Vec<String>> {
    if !paths
        .iter()
        .any(|path| in_dir(path, crate::design_system_artifact::SYSTEM))
    {
        return Ok(Vec::new());
    }
    let mut inputs = Source::Commit {
        repo: top,
        rev: commit,
    }
    .design_system_inputs()?;
    inputs.push(boards::runtime::RECORD.to_string());
    Ok(inputs)
}

/// Whether <path> is <input> or a file below it.
fn is_at(path: &str, input: &str) -> bool {
    path == input || in_dir(path, input)
}

/// The id that keys <tree>'s shots in the picture folder for a shots run made
/// in <environment> (app_platform.rs): 16 hex digits of the two together.
/// builds passes it to the shots program as the app tree, so the one id means
/// everything that draws the shots, the app's source and the host software,
/// and a shot cached in one environment is never reused in another.
pub fn in_environment(tree: &str, environment: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tree.as_bytes());
    hasher.update([0]);
    hasher.update(environment.as_bytes());
    crate::text::hex(&hasher.finalize()[..8])
}

/// The files every crate builds from, which the workspace has at its top:
/// the root files `builds checks` counts as every crate's, the lock, and
/// cargo's configuration folder.
fn is_workspace_input(path: &str) -> bool {
    crate::checks::ROOT_CONFIGS.contains(&path)
        || path == crate::checks::LOCK
        || path.starts_with(".cargo/")
}

/// Whether <path> is a workspace input, or a file of one of <crate_dirs>.
fn is_rust_input(path: &str, crate_dirs: &BTreeSet<String>) -> bool {
    !crate_dirs.is_empty()
        && (is_workspace_input(path) || crate_dirs.iter().any(|dir| in_dir(path, dir)))
}

/// The folders of the lane's seed crates and of every crate they depend on.
/// Empty when the lane has no seed.
fn closure_dirs(
    top: &Path,
    commit: &str,
    paths: &[&str],
    matched: &dyn Fn(&str) -> bool,
) -> Result<BTreeSet<String>> {
    // Each crate's folder and the names of the workspace crates it names as
    // dependencies (a name that is no workspace crate is a registry crate,
    // which Cargo.lock holds).
    let mut folders: BTreeMap<String, String> = BTreeMap::new();
    let mut named: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in paths {
        let Some(dir) = path.strip_suffix("Cargo.toml") else {
            continue;
        };
        if !(dir.is_empty() || dir.ends_with('/')) {
            continue;
        }
        let text = run_git(top, &["show", &format!("{commit}:{path}")])?;
        let manifest: toml::Table = match text.parse() {
            Ok(table) => table,
            Err(error) => fail!("{path} at {commit} is not TOML: {error}"),
        };
        let name = match manifest.get("package") {
            Some(package) => match package.get("name").and_then(|name| name.as_str()) {
                Some(name) => name,
                None => fail!("{path} at {commit} has a [package] with no name"),
            },
            // A workspace manifest that holds no crate of its own.
            None if manifest.contains_key("workspace") => continue,
            None => fail!("{path} at {commit} has neither [package] nor [workspace]"),
        };
        let mut dependencies = BTreeSet::new();
        dependency_names(&manifest, &mut dependencies);
        folders.insert(name.to_string(), dir.trim_end_matches('/').to_string());
        named.insert(name.to_string(), dependencies);
    }
    let mut pending: Vec<String> = Vec::new();
    for (name, dir) in &folders {
        if !dir.is_empty() && paths.iter().any(|path| in_dir(path, dir) && matched(path)) {
            pending.push(name.clone());
        }
    }
    // The crates a matched script builds with `-p <crate>`.
    for path in paths
        .iter()
        .filter(|path| path.ends_with(".sh") && matched(path))
    {
        let text = run_git(top, &["show", &format!("{commit}:{path}")])?;
        for line in text.lines() {
            let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
            pending.extend(
                crate::cargo_run_crate::named(&words)
                    .into_iter()
                    .filter(|name| folders.contains_key(name)),
            );
        }
    }
    let mut closure = BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !closure.insert(name.clone()) {
            continue;
        }
        pending.extend(
            named[&name]
                .iter()
                .filter(|dependency| folders.contains_key(*dependency))
                .cloned(),
        );
    }
    Ok(closure.iter().map(|name| folders[name].clone()).collect())
}

fn in_dir(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Adds to <names> the package each dependency of <manifest> names, in its
/// dependency tables and those under `target`.
fn dependency_names(manifest: &toml::Table, names: &mut BTreeSet<String>) {
    for table in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(dependencies) = manifest.get(table).and_then(|value| value.as_table()) else {
            continue;
        };
        for (key, value) in dependencies {
            let renamed = value.get("package").and_then(|package| package.as_str());
            names.insert(renamed.unwrap_or(key).to_string());
        }
    }
    if let Some(targets) = manifest.get("target").and_then(|value| value.as_table()) {
        for target in targets.values().filter_map(|value| value.as_table()) {
            dependency_names(target, names);
        }
    }
}

#[cfg(test)]
#[path = "app_tree_tests.rs"]
mod tests;

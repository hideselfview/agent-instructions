//! Separate builds inside one build folder, each aged on its own.
//!
//! A folder in `.build-dirs` is aged as a whole: its newest write anywhere
//! inside is its last use. When one folder holds several builds, a live one
//! keeps a stale one beside it young for good: bae-macos/bae/.build held an
//! Xcode tree from October 25 beside one built on November 3, 2.8 GB nothing
//! used, and no pass ever deleted it. So `setup` and `prune` find the builds
//! inside each folder and delete each one no build has used for
//! `BUILDS_UNUSED_DAYS` days, the way cargo's unused build copies go. Two kinds
//! are recognized by what their tool writes:
//!
//!   an Xcode tree      info.plist beside Logs, and Build or ModuleCache.noindex,
//!                      one or two levels below the folder
//!                      (bae-macos/bae/.build/unsigned)
//!   a SwiftPM build    <triple>/debug or <triple>/release inside a folder that
//!                      holds workspace-state.json (BaeKit/.build/arm64-apple-
//!                      macosx/debug); checkouts, repositories and artifacts
//!                      belong to every build and stay
//!
//! A build's last use is the newest write anywhere inside it. xcodebuild writes
//! a log under Logs/Build on every run, and `swift build` rewrites
//! plugin-tools-description.json and the output file maps even when nothing
//! changed (both measured), so an up-to-date build still counts as use. A
//! `dotnet build` that finds everything up to date writes nothing (measured),
//! and Gradle keeps no build per variant to delete, so those folders are not
//! split here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::evict::{Recreate, Removal, evict};
use crate::fail;
use crate::fail::Result;
use crate::prune::Run;
use crate::text::{human, note};
use crate::tool::Tool;
use crate::usage::Measure;

/// How many levels below a build folder an Xcode tree may sit.
const XCODE_DEPTH: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Xcode,
    SwiftPm,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Xcode => "xcode",
            Kind::SwiftPm => "swiftpm",
        }
    }
}

/// One build inside a build folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    pub path: PathBuf,
    pub kind: Kind,
}

/// Is a folder holding exactly these entries an Xcode DerivedData tree?
pub fn is_xcode_tree(names: &HashSet<String>) -> bool {
    let has = |name: &str| names.contains(name);
    has("info.plist") && has("Logs") && (has("Build") || has("ModuleCache.noindex"))
}

/// Is a folder holding exactly these entries a SwiftPM `.build` folder?
pub fn is_swiftpm_folder(names: &HashSet<String>) -> bool {
    let has = |name: &str| names.contains(name);
    has("workspace-state.json") && (has("checkouts") || has("repositories") || has("artifacts"))
}

fn listing(dir: &Path) -> Result<Vec<(String, bool)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else { continue };
        // A link is never a build of its own: SwiftPM's `.build/debug` points
        // at the real one.
        found.push((entry.file_name().to_string_lossy().into_owned(), kind.is_dir()));
    }
    found.sort();
    Ok(found)
}

/// The builds inside <folder>, a build folder of a checkout's manifest.
pub fn trees_in(folder: &Path) -> Result<Vec<Tree>> {
    let mut found = Vec::new();
    let names = listing(folder)?;
    let set: HashSet<String> = names.iter().map(|(name, _)| name.clone()).collect();
    if is_swiftpm_folder(&set) {
        for (name, is_dir) in &names {
            if !is_dir || matches!(name.as_str(), "checkouts" | "repositories" | "artifacts" | "plugins") {
                continue;
            }
            let triple = folder.join(name);
            for (config, is_dir) in listing(&triple)? {
                if is_dir && (config == "debug" || config == "release") {
                    found.push(Tree { path: triple.join(config), kind: Kind::SwiftPm });
                }
            }
        }
        return Ok(found);
    }
    // A cargo folder holds tens of thousands of entries and its own pass
    // (units) ages its build copies.
    if !set.contains(".rustc_info.json") {
        xcode_trees(folder, 0, &mut found)?;
    }
    Ok(found)
}

fn xcode_trees(dir: &Path, depth: usize, found: &mut Vec<Tree>) -> Result<()> {
    if depth >= XCODE_DEPTH {
        return Ok(());
    }
    for (name, is_dir) in listing(dir)? {
        if !is_dir {
            continue;
        }
        let child = dir.join(name);
        let inside: HashSet<String> = listing(&child)?.into_iter().map(|(name, _)| name).collect();
        if is_xcode_tree(&inside) {
            found.push(Tree { path: child, kind: Kind::Xcode });
        } else {
            xcode_trees(&child, depth + 1, found)?;
        }
    }
    Ok(())
}

/// Did no build use a tree measuring <measured> for more than the rules' days?
pub fn unused(tool: &Tool, measured: &Measure) -> bool {
    measured.newest.is_some_and(|newest| tool.now - newest > (tool.rules.unused_days * 86400) as i64)
}

/// Deletes each build inside <folders> that no build used for the rules' days,
/// or says it would on a dry run. Returns, per folder, the KiB a dry run
/// would free, for the caller measuring the folder against its cap.
pub fn prune_trees(tool: &Tool, folders: &[PathBuf], run: Run) -> Result<Vec<(PathBuf, u64)>> {
    let mut all = Vec::new();
    for folder in folders {
        for tree in trees_in(folder)? {
            all.push((folder.clone(), tree));
        }
    }
    let paths: Vec<PathBuf> = all.iter().map(|(_, tree)| tree.path.clone()).collect();
    let measures = tool.measure_all(&paths)?;
    let mut freed = Vec::new();
    for ((folder, tree), measured) in all.into_iter().zip(measures) {
        if !unused(tool, &measured) {
            continue;
        }
        let reason = format!("{}, no build used it for {} days", human(measured.kib), tool.rules.unused_days);
        if run.dry() {
            note(format!("would delete {} ({reason})", tree.path.display()));
            freed.push((folder, measured.kib));
        } else if evict(&tool.places, &tree.path, Recreate::No, Removal::Background)? {
            note(format!("deleted {} ({reason}); the next build makes it again", tree.path.display()));
        } else {
            note(format!("in use, kept: {} ({reason})", tree.path.display()));
        }
    }
    Ok(freed)
}

#[cfg(test)]
#[path = "trees_tests.rs"]
mod tests;

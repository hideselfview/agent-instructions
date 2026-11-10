//! `builds setup`: links a checkout's build paths into its build root, then
//! bounds what builds left behind and keeps the free-space floor.
//!
//! Each path the repo's `.build-dirs` lists is a symlink in the checkout to
//! the same path under the checkout's build root (target ->
//! /Volumes/Builds/bae/main/target). The path a tool sees never changes:
//! cargo, xcodebuild, Gradle, dotnet and lake always write through
//! <checkout>/<path>, so sccache keys and cargo fingerprints stay valid however
//! often the folder behind the link is deleted and recreated. Two checkouts
//! never share a folder, so their builds never wait on each other's locks.

use std::path::{Path, PathBuf};

use crate::checkout::{Checkout, Root, absolute_link_target, worktrees_of};
use crate::evict::{Recreate, Removal, evict};
use crate::fail;
use crate::fail::{Context, Result};
use crate::floor::keep_floor;
use crate::manifest::{MANIFEST, build_dirs, has_manifest};
use crate::prune::{
    Run, prune_caches, prune_cargo_git, prune_derived_data, prune_root, prune_temp_builds,
    prune_test_temps,
};
use crate::text::note;
use crate::tool::Tool;

/// May <checkout> delete <folder>, the folder its link points at today?
/// Never a folder inside another live checkout's build root. A folder from an
/// older layout that another checkout's link also points at is the main
/// checkout's to delete; a side checkout leaves it and starts its own.
fn may_delete(tool: &Tool, folder: &Path, checkout: &Checkout) -> Result<bool> {
    let repo_dir = tool.places.root.join(checkout.repo());
    if let Ok(inside) = folder.strip_prefix(&repo_dir) {
        let mut parts = inside.components();
        if let (Some(owner), Some(_)) = (parts.next(), parts.next()) {
            let owner_root = repo_dir.join(owner);
            if let Some(root) = Root::read(&tool.places, &owner_root)?
                && root.top != checkout.top && root.live.is_some() {
                    return Ok(false);
                }
        }
    }
    if checkout.is_main() {
        return Ok(true);
    }
    let manifest = build_dirs(&checkout.top)?;
    for worktree in worktrees_of(&checkout.top)? {
        if worktree == checkout.top {
            continue;
        }
        for entry in &manifest {
            let link = worktree.join(&entry.path);
            if link.is_symlink() && absolute_link_target(&link)? == folder {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Makes <link> a symlink to <dest> and makes sure <dest> exists. A folder at
/// any other place — behind a link to an older location (when may_delete says
/// it is this checkout's), or a real directory a tool left before the link
/// existed — is deleted, never moved: Xcode's DerivedData records its own
/// absolute path, so a moved one fails to find its packages. The next build
/// refills <dest>, cargo mostly from sccache.
fn place(tool: &Tool, link: &Path, dest: &Path, checkout: &Checkout) -> Result<()> {
    let shown = link.strip_prefix(&checkout.top).unwrap_or(link).display().to_string();
    if link.is_symlink() {
        let current = absolute_link_target(link)?;
        if current != dest {
            if current.is_dir() && may_delete(tool, &current, checkout)? {
                if !evict(&tool.places, &current, Recreate::No, Removal::Background)? {
                    fail!("a build is using {}; run builds setup again once it ends", current.display());
                }
                note(format!("deleted {}, a folder from an older layout", current.display()));
            }
            replace_link(dest, link)?;
            note(format!("linked {shown} -> {}", dest.display()));
        }
    } else if link.exists() {
        if !evict(&tool.places, link, Recreate::No, Removal::Background)? {
            fail!("a build is using {shown}; run builds setup again once it ends");
        }
        note(format!("deleted {shown}, a real directory; it builds into {} now", dest.display()));
        replace_link(dest, link)?;
        note(format!("linked {shown} -> {}", dest.display()));
    } else {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).context(|| format!("creating {}", parent.display()))?;
        }
        replace_link(dest, link)?;
        note(format!("linked {shown} -> {}", dest.display()));
    }
    // A link whose folder was deleted breaks every tool that writes through
    // it (cargo os error 20, xcodebuild rc 74, Gradle's file hash cache,
    // dotnet MSB3021, lake error 17); recreating the folder is all any of
    // them need.
    std::fs::create_dir_all(dest).context(|| format!("creating {}", dest.display()))
}

/// Points <link> at <dest>, replacing whatever link is there in one rename,
/// so no tool ever finds the path missing.
fn replace_link(dest: &Path, link: &Path) -> Result<()> {
    let name = link.file_name().expect("a link has a name").to_string_lossy();
    let staged: PathBuf = link.with_file_name(format!(".{name}.builds-link.{}", std::process::id()));
    std::os::unix::fs::symlink(dest, &staged).context(|| format!("linking {}", staged.display()))?;
    std::fs::rename(&staged, link).context(|| format!("linking {}", link.display()))
}

/// `builds setup [<checkout>]`.
pub fn cmd_setup(tool: &Tool, args: &[String]) -> Result<()> {
    let arg = match args {
        [] => ".",
        [arg] => arg.as_str(),
        _ => fail!("usage: builds setup [<checkout>]"),
    };
    setup(tool, &Checkout::find(arg)?)
}

pub fn setup(tool: &Tool, checkout: &Checkout) -> Result<()> {
    if !has_manifest(&checkout.top) {
        note(format!("{} has no {MANIFEST}; nothing to link", checkout.top.display()));
        return Ok(());
    }
    tool.places.require_volume()?;
    checkout.require_repo_name(&tool.places)?;
    let root = checkout.build_root(&tool.places);
    std::fs::create_dir_all(&root).context(|| format!("creating {}", root.display()))?;
    let marker = root.join(".checkout");
    std::fs::write(&marker, format!("{}\n", checkout.top.display()))
        .context(|| format!("writing {}", marker.display()))?;
    for entry in build_dirs(&checkout.top)? {
        place(tool, &checkout.top.join(&entry.path), &root.join(&entry.path), checkout)?;
        if entry.path == "target"
            && let Some(dir) = std::env::var_os("CARGO_TARGET_DIR").filter(|dir| !dir.is_empty()) {
                note(format!(
                    "CARGO_TARGET_DIR={} overrides {}/target for cargo run here",
                    dir.to_string_lossy(),
                    checkout.top.display()
                ));
            }
    }
    prune_root(tool, &root, checkout, Run::Real, 0)?;
    prune_derived_data(tool, Run::Real, 0)?;
    prune_caches(tool, Run::Real)?;
    prune_test_temps(tool, Run::Real)?;
    prune_temp_builds(tool, Run::Real)?;
    let roots = tool.roots()?;
    prune_cargo_git(tool, &roots, Run::Real)?;
    keep_floor(tool, &roots)
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;

//! `builds setup`: links a checkout's build paths into its build root,
//! bounds what builds left behind and keeps the free-space floor, then runs
//! the checkout's `prepare` programs, installs the Lean toolchains its Lean
//! packages pin (lean.rs) and puts the builds of its tools in place
//! (built_tools.rs).
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
use crate::floor::{Keeper, keep_floor};
use crate::host::Ssh;
use crate::manifest::{MANIFEST, build_dirs, has_manifest};
use crate::prune::{
    Run, prune_caches, prune_cargo_git, prune_derived_data, prune_installed, prune_root,
    prune_scratchpads, prune_temp_builds, prune_test_temps,
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
                && root.top != checkout.top
                && root.live.is_some()
            {
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
    let shown = link
        .strip_prefix(&checkout.top)
        .unwrap_or(link)
        .display()
        .to_string();
    if link.is_symlink() {
        let current = absolute_link_target(link)?;
        if current != dest {
            if current.is_dir() && may_delete(tool, &current, checkout)? {
                if !evict(&tool.places, &current, Recreate::No, Removal::Background)? {
                    fail!(
                        "a build is using {}; run builds setup again once it ends",
                        current.display()
                    );
                }
                note(format!(
                    "deleted {}, a folder from an older layout",
                    current.display()
                ));
            }
            replace_link(dest, link)?;
            note(format!("linked {shown} -> {}", dest.display()));
        }
    } else if link.exists() {
        if !evict(&tool.places, link, Recreate::No, Removal::Background)? {
            fail!("a build is using {shown}; run builds setup again once it ends");
        }
        note(format!(
            "deleted {shown}, a real directory; it builds into {} now",
            dest.display()
        ));
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
pub fn replace_link(dest: &Path, link: &Path) -> Result<()> {
    let name = link
        .file_name()
        .expect("a link has a name")
        .to_string_lossy();
    let staged: PathBuf = link.with_file_name(format!(
        ".{name}.builds-link.{}",
        crate::text::staged_suffix()
    ));
    std::os::unix::fs::symlink(dest, &staged)
        .context(|| format!("linking {}", staged.display()))?;
    std::fs::rename(&staged, link).context(|| format!("linking {}", link.display()))
}

/// Whether setup prepares the checkout for its builds: runs its `prepare`
/// programs, installs its Lean toolchains, puts its tools' builds in place
/// and copies them to the hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prepare {
    Yes,
    /// For a host's job that a pre-commit hook check started: a hook
    /// installs, fetches and builds nothing.
    No,
}

/// `builds setup [--no-prepare] [<checkout>]`.
pub fn cmd_setup(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    let (prepare, rest) = match args {
        [flag, rest @ ..] if flag == "--no-prepare" => (Prepare::No, rest),
        rest => (Prepare::Yes, rest),
    };
    let arg = match rest {
        [] => ".",
        [arg] => arg.as_str(),
        _ => fail!("usage: builds setup [--no-prepare] [<checkout>]"),
    };
    setup_with(tool, ssh, &Checkout::find(arg)?, prepare)
}

/// Sets <checkout> up and prepares it (`Prepare::Yes`).
pub fn setup(tool: &Tool, ssh: &Ssh, checkout: &Checkout) -> Result<()> {
    setup_with(tool, ssh, checkout, Prepare::Yes)
}

pub fn setup_with(tool: &Tool, ssh: &Ssh, checkout: &Checkout, prepare: Prepare) -> Result<()> {
    if !has_manifest(&checkout.top) {
        note(format!(
            "{} has no {MANIFEST}; nothing to link",
            checkout.top.display()
        ));
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
        place(
            tool,
            &checkout.top.join(&entry.path),
            &root.join(&entry.path),
            checkout,
        )?;
        if entry.path == "target"
            && let Some(dir) = std::env::var_os("CARGO_TARGET_DIR").filter(|dir| !dir.is_empty())
        {
            note(format!(
                "CARGO_TARGET_DIR={} overrides {}/target for cargo run here",
                dir.to_string_lossy(),
                checkout.top.display()
            ));
        }
    }
    prune_root(tool, &root, checkout, Run::Real, 0, 0)?;
    prune_derived_data(tool, Run::Real, 0)?;
    prune_caches(tool, Run::Real)?;
    crate::shots_cache::prune(tool, Run::Real)?;
    crate::review::texts::prune(tool, Run::Real)?;
    prune_test_temps(tool, Run::Real)?;
    prune_temp_builds(tool, Run::Real)?;
    prune_scratchpads(tool, Run::Real)?;
    let roots = tool.roots()?;
    prune_cargo_git(tool, &roots, Run::Real)?;
    prune_installed(tool, &roots, Run::Real)?;
    crate::built_tools::prune_tools(&tool.places, &roots, Run::Real)?;
    keep_floor(tool, &roots, Keeper::Setup, Run::Real)?;
    if prepare == Prepare::No {
        return Ok(());
    }
    // Last, so what they install or build lands after the floor is kept.
    self::prepare(&checkout.top)?;
    crate::lean::ensure(&tool.places, &checkout.top)?;
    crate::built_tools::ensure_all(&tool.places, ssh, &checkout.top)?;
    crate::built_tools::send_to_hosts(&tool.places, ssh, &checkout.top)
}

/// Runs each program the `prepare` lines of <top>'s manifest name, in its
/// top folder, its output on stderr: each puts in place what the checkout's
/// builds need beyond their folders and finishes at once when that is in
/// place. Setup runs them, and so does `builds cargo` on the Mac that runs
/// cargo, unless a pre-commit hook check started it: the hook's checks only
/// check that the pieces are there.
/// A program never runs `builds cargo`, which would run it again. The first
/// that fails stops the rest and fails, naming it.
pub fn prepare(top: &Path) -> Result<()> {
    for program in crate::manifest::prepare_programs(top)? {
        let path = top.join(&program);
        let mut command = std::process::Command::new(&path);
        // A `builds cargo` that a hook check runs has git's variables for the
        // commit's index, which would point the program's git commands there.
        for name in crate::checkout::git_local_env_vars()? {
            command.env_remove(name);
        }
        let status = command
            .current_dir(top)
            .stdin(std::process::Stdio::null())
            .stdout(crate::text::stderr_for_child()?)
            .status()
            .context(|| format!("running {}", path.display()))?;
        if !status.success() {
            fail!("{program}, which {MANIFEST} names to prepare the checkout, failed ({status})");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;

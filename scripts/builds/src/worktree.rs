//! Side worktrees: adding one, retiring one, and recording a repo's app
//! checkout.
//!
//! By default a side worktree builds host Rust only, and a repo with app
//! builds keeps them warm in main and in one side worktree per lane (mac,
//! android), its app checkout, which agents take turns in by holding the
//! lane's lock, <root>/<repo>/app.lock for the mac lane; see
//! principles/worktree-build-priming.md. Nothing here enforces who builds
//! what: the caps bound whatever a checkout builds, and the floor bounds every
//! checkout but main and the folders an app checkout's lane builds into.

use std::path::Path;

use crate::checkout::{Checkout, git, git_text};
use crate::claim::{Claim, require_one_line, write_claim};
use crate::evict::{Recreate, Removal, evict};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::free_worktrees::find_free;
use crate::lane::Lane;
use crate::setup::setup;
use crate::text::note;
use crate::tool::Tool;

fn run_checked(command: &mut std::process::Command, what: &str) -> Result<()> {
    let status = command.status().context(|| format!("running {what}"))?;
    if !status.success() {
        // The command printed why.
        return Err(Fail::Message(format!("{what} exited {}", status.code().unwrap_or(-1))));
    }
    Ok(())
}

/// `builds new [<repo>] [-b <branch>] [--for <who>]`: claims for <who> (default
/// "builds new") and hands out a free side
/// worktree (`find_free`) checked out afresh from origin/main, or adds the next
/// side-N when none is free; links it, runs its post-checkout, and prints its
/// path.
pub fn cmd_new(tool: &Tool, args: &[String]) -> Result<()> {
    let mut repo_path = ".".to_string();
    let mut branch = None;
    let mut who = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-b" | "--branch" => branch = rest.next().cloned(),
            "--for" => who = rest.next().cloned(),
            other if other.starts_with('-') => fail!("new: unknown option {other}"),
            other => repo_path = other.to_string(),
        }
    }
    if let Some(who) = &who {
        require_one_line("--for <who>", who)?;
    }
    tool.places.require_volume()?;
    let main = Checkout::find(&repo_path)?.main;
    // Held from picking a worktree to claiming it, so two calls never get the
    // same one; released when the call ends.
    let _picking = lock_picking(&main)?;
    run_checked(git(&main).args(["fetch", "-q", "origin"]), "git fetch")?;
    let who = who.unwrap_or_else(|| "builds new".to_string());
    let checkout = match find_free(tool, &main)? {
        Some(free) => {
            write_claim(&free, &who, tool.now, None, None)?;
            if let Err(error) = reuse(&free, branch.as_deref()).and_then(|()| prepare(tool, &free)) {
                let _ = std::fs::remove_file(free.claim_file());
                return Err(error);
            }
            free
        }
        None => {
            let dir = add_new(tool, &main, branch.as_deref())?;
            let checkout = Checkout::find(&dir.display().to_string())?;
            write_claim(&checkout, &who, tool.now, None, None)?;
            prepare(tool, &checkout)?;
            checkout
        }
    };
    let dir = &checkout.top;
    println!("{}", dir.display());
    Ok(())
}

/// An exclusive lock on <main>'s repo for picking a worktree (flock on a file
/// in the common git dir), held until the returned file is dropped.
fn lock_picking(main: &Path) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    let path = main.join(".git/builds-new.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .context(|| format!("opening {}", path.display()))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        fail!("locking {}: {}", path.display(), std::io::Error::last_os_error());
    }
    Ok(file)
}

/// Adds the next side-N worktree of <main>'s repo, from origin/main.
fn add_new(tool: &Tool, main: &Path, branch: Option<&str>) -> Result<std::path::PathBuf> {
    let repo = crate::checkout::file_name(main);
    let parent = main.parent().expect("a main checkout has a folder").join(format!("{repo}-worktrees"));
    let mut n = 1;
    while parent.join(format!("side-{n}")).exists() || tool.places.root.join(&repo).join(format!("side-{n}")).exists() {
        n += 1;
    }
    let dir = parent.join(format!("side-{n}"));
    std::fs::create_dir_all(&parent).context(|| format!("creating {}", parent.display()))?;
    // The hooks the common .git points at may belong to another checkout;
    // this one's own post-checkout runs in `prepare`, after its build paths
    // are linked.
    let mut add = git(main);
    add.args(["-c", "core.hooksPath=/dev/null", "worktree", "add", "-q"]);
    match branch {
        Some(branch) => add.arg("-b").arg(branch),
        None => add.arg("--detach"),
    };
    add.arg(&dir).arg("origin/main");
    run_checked(&mut add, "git worktree add")?;
    Ok(dir)
}

/// Checks a fresh branch out of origin/main in the free worktree <free>, or
/// detaches HEAD there when no branch is named. Nothing in it is lost: a free
/// worktree is clean and holds no commit missing from origin/main.
fn reuse(free: &Checkout, branch: Option<&str>) -> Result<()> {
    let mut checkout = git(&free.top);
    checkout.args(["-c", "core.hooksPath=/dev/null", "checkout", "-q"]);
    match branch {
        Some(branch) => checkout.arg("-b").arg(branch),
        None => checkout.arg("--detach"),
    };
    checkout.arg("origin/main");
    run_checked(&mut checkout, "git checkout")
}

/// Links the worktree's build paths, installs the repo's hooks when it has
/// `.checks`, and runs its own post-checkout.
fn prepare(tool: &Tool, checkout: &Checkout) -> Result<()> {
    let dir = &checkout.top;
    setup(tool, checkout)?;
    crate::warm::fill(tool, checkout)?;
    if dir.join(crate::checks::CONFIG).is_file() {
        crate::install_hook::install(dir)?;
    }
    let hook = dir.join("scripts/hooks/post-checkout");
    if is_executable(&hook) {
        let head = git_text(git(dir).args(["rev-parse", "HEAD"]), "read HEAD")?;
        run_checked(
            std::process::Command::new(&hook)
                .args(["0000000000000000000000000000000000000000", head.trim(), "1"])
                .current_dir(dir),
            "scripts/hooks/post-checkout",
        )?;
    }
    Ok(())
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// `builds retire <checkout>`: removes a clean, unclaimed side worktree and
/// its build root.
pub fn cmd_retire(tool: &Tool, args: &[String]) -> Result<()> {
    let [arg] = args else { fail!("usage: builds retire <checkout>") };
    let checkout = Checkout::find(arg)?;
    let top = checkout.top.display();
    if !checkout.is_side() {
        let what = if checkout.is_main() { "the main checkout" } else { "not one of the repo's side worktrees" };
        fail!("{top} is {what}; only side worktrees retire");
    }
    let repo = checkout.repo();
    if let Some(lane) = tool.places.app_lane_of(&repo, &checkout.name())? {
        fail!(
            "{top} is one of {repo}'s {} app checkouts; builds app-checkout remove {} {top} first",
            lane.word(),
            lane.word()
        );
    }
    retire_checkout(tool, &checkout)
}

/// Removes the clean, unclaimed side worktree <checkout> and its build root.
pub fn retire_checkout(tool: &Tool, checkout: &Checkout) -> Result<()> {
    let top = checkout.top.display();
    let root = checkout.build_root(&tool.places);
    if let Some(claim) = Claim::read(&checkout.claim_file())? {
        fail!("{top} is claimed by {}; builds release it first", claim.text());
    }
    let dirty = git_text(git(&checkout.top).args(["status", "--porcelain"]), "read the worktree's status")?;
    if !dirty.is_empty() {
        fail!("{top} has uncommitted or untracked files; commit or remove them first");
    }
    run_checked(git(&checkout.main).arg("worktree").arg("remove").arg(&checkout.top), "git worktree remove")?;
    if root.is_dir() && !evict(&tool.places, &root, Recreate::No, Removal::Background)? {
        fail!(
            "worktree removed, but a process holds files in {}; run builds prune once it exits",
            root.display()
        );
    }
    note(format!("retired {top} and deleted {}", root.display()));
    Ok(())
}

/// `builds app-checkout mac|android <checkout>`: adds the side worktree to its
/// repo's app checkouts for the lane; `builds app-checkout remove mac|android
/// <checkout>` takes it out again, while no build holds its lock; `builds
/// app-checkout reset <repo> <checkout>` puts one on origin's main (app_reset.rs).
pub fn cmd_app_checkout(tool: &Tool, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: builds app-checkout mac|android <checkout> | builds app-checkout remove mac|android <checkout> | builds app-checkout reset <repo> <checkout> [--matching <rev>]";
    let reset = match args {
        [word, repo, name] if word == "reset" => Some((repo, name, None)),
        [word, repo, name, flag, rev] if word == "reset" && flag == "--matching" => Some((repo, name, Some(rev.as_str()))),
        _ => None,
    };
    if let Some((repo, name, matching)) = reset {
        tool.places.require_volume()?;
        return crate::app_reset::cmd_reset(&tool.places, repo, name, matching);
    }
    let (removing, lane, arg) = match args {
        [word, lane, arg] if word == "remove" => (true, lane, arg),
        [lane, arg] => (false, lane, arg),
        _ => fail!("{USAGE}"),
    };
    let Some(lane) = Lane::parse(lane) else { fail!("{USAGE}") };
    let checkout = Checkout::find(arg)?;
    if checkout.is_main() {
        fail!(
            "{} is the main checkout, which keeps its builds warm already; an app checkout is a side worktree",
            checkout.top.display()
        );
    }
    if !checkout.is_side() {
        fail!("{} is not one of the repo's side worktrees; an app checkout is one", checkout.top.display());
    }
    tool.places.require_volume()?;
    checkout.require_repo_name(&tool.places)?;
    let repo = checkout.repo();
    let name = checkout.name();
    let mut set = tool.places.app_checkouts_of(&repo, lane)?;
    if removing {
        if !set.contains(&name) {
            fail!("{} is not one of {repo}'s {} app checkouts", checkout.top.display(), lane.word());
        }
        let lock = tool.places.app_lock_file(&repo, lane, &name);
        if crate::queue::is_held(&lock)? {
            fail!("a build holds {}'s lock; remove it once that build ends", checkout.top.display());
        }
        set.retain(|other| *other != name);
        tool.places.write_app_checkouts(&repo, lane, &set)?;
        match std::fs::remove_file(&lock) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                fail!("removing {}: {error}", lock.display())
            }
            _ => {}
        }
        crate::queue::repoint(&tool.places, &repo, lane);
        note(format!("removed {} from {repo}'s {} app checkouts", checkout.top.display(), lane.word()));
        return Ok(());
    }
    if let Some(other) = tool.places.app_lane_of(&repo, &name)?.filter(|other| *other != lane) {
        fail!(
            "{} is {repo}'s {} app checkout already; the lanes exist so that their builds share no checkout",
            checkout.top.display(),
            other.word()
        );
    }
    if !set.contains(&name) {
        set.push(name);
        tool.places.write_app_checkouts(&repo, lane, &set)?;
        crate::queue::repoint(&tool.places, &repo, lane);
    }
    note(format!(
        "{} is one of {repo}'s {} app checkouts ({} in all, in {}); the free-space floor keeps the folders its lane builds into",
        checkout.top.display(),
        lane.word(),
        set.len(),
        tool.places.app_checkouts_file(&repo, lane).display()
    ));
    Ok(())
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;

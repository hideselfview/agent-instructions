//! Side worktrees: adding one, retiring one, and recording a repo's app
//! checkout.
//!
//! By default a side worktree builds host Rust only, and a repo with app
//! builds keeps them built in main and in its app checkouts, one or more side
//! worktrees per lane (mac, android), which agents take turns in by holding a
//! checkout's lock, <root>/<repo>/app.locks/<checkout> for the mac lane; see
//! APP BUILDS in help.txt. Nothing here enforces who builds what: the caps
//! bound whatever a checkout builds, and the floor bounds every checkout but
//! main and the folders an app checkout's lane builds into.

use std::path::Path;

use crate::app_remote::RemoteApp;
use crate::checkout::{Checkout, git, git_text};
use crate::claim::{Claim, require_one_line, require_project_named, set_project, write_claim};
use crate::evict::{Recreate, Removal, evict};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::free_worktrees::find_free;
use crate::host::Ssh;
use crate::lane::Lane;
use crate::setup::setup;
use crate::text::note;
use crate::tool::Tool;

fn run_checked(command: &mut std::process::Command, what: &str) -> Result<()> {
    let status = command.status().context(|| format!("running {what}"))?;
    if !status.success() {
        // The command printed why.
        return Err(Fail::Message(format!(
            "{what} exited {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

/// `builds new [<repo>] [-b <branch> | --resume <branch>] [--for <who>]
/// [--project P]`: claims for <who> (default "builds new"), records the
/// project P the work is in (which a repo two projects hold requires) and
/// hands out a free side worktree (`find_free`) checked out afresh from
/// origin/main, or adds the next side-N when none is free; links it, runs its
/// post-checkout, and prints its path. --resume checks out the existing
/// branch instead, one no worktree checks out: work left on a branch goes
/// back into a worktree this way.
pub fn cmd_new(tool: &Tool, args: &[String]) -> Result<()> {
    let mut repo_path = ".".to_string();
    let mut branch = None;
    let mut resume = None;
    let mut who = None;
    let mut project = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-b" | "--branch" => branch = rest.next().cloned(),
            "--resume" => resume = rest.next().cloned(),
            "--for" => who = rest.next().cloned(),
            "--project" => project = rest.next().cloned(),
            other if other.starts_with('-') => fail!("new: unknown option {other}"),
            other => repo_path = other.to_string(),
        }
    }
    if let Some(who) = &who {
        require_one_line("--for <who>", who)?;
    }
    let start = match (branch, resume) {
        (Some(_), Some(_)) => fail!("new: name one branch, with -b or --resume"),
        (Some(branch), None) => Start::Fresh(Some(branch)),
        (None, Some(branch)) => Start::Resume(branch),
        (None, None) => Start::Fresh(None),
    };
    tool.places.require_volume()?;
    let main = Checkout::find(&repo_path)?.main;
    if let Start::Resume(branch) = &start {
        require_resumable(&main, branch)?;
    }
    // Held from picking a worktree to claiming it, so two calls never get the
    // same one; released when the call ends.
    let _picking = lock_picking(&main)?;
    run_checked(git(&main).args(["fetch", "-q", "origin"]), "git fetch")?;
    let repo = Checkout::find(&main.display().to_string())?.repo();
    if let Some(project) = &project
        && !crate::lists::store::projects_holding(&tool.places, &repo)?.contains(project)
    {
        fail!("new: project {project} holds no repo {repo}");
    }
    require_project_named(&tool.places, &repo, project.as_deref())?;
    let who = who.unwrap_or_else(|| "builds new".to_string());
    let checkout = match find_free(tool, &main)? {
        Some(free) => {
            write_claim(&free, &who, tool.now, None, None)?;
            set_project(&tool.places, &free, project.as_deref())?;
            if let Err(error) = reuse(&free, &start).and_then(|()| prepare(tool, &free)) {
                let _ = std::fs::remove_file(free.claim_file());
                let _ = std::fs::remove_file(free.project_file());
                return Err(error);
            }
            free
        }
        None => {
            let dir = add_new(tool, &main, &start)?;
            let checkout = Checkout::find(&dir.display().to_string())?;
            write_claim(&checkout, &who, tool.now, None, None)?;
            set_project(&tool.places, &checkout, project.as_deref())?;
            prepare(tool, &checkout)?;
            checkout
        }
    };
    let dir = &checkout.top;
    println!("{}", dir.display());
    Ok(())
}

/// What a handed-out worktree checks out.
enum Start {
    /// origin/main, on a new branch of this name or detached.
    Fresh(Option<String>),
    /// An existing branch no worktree checks out.
    Resume(String),
}

/// Refuses to resume <branch> in <main>'s repo when it does not exist or a
/// worktree checks it out: git would refuse the checkout after `builds new`
/// claimed the worktree.
fn require_resumable(main: &Path, branch: &str) -> Result<()> {
    if !crate::checkout::branch_exists(main, branch)? {
        fail!(
            "new: {} has no branch {branch} to resume",
            crate::checkout::file_name(main)
        );
    }
    if let Some(holder) = crate::checkout::worktree_list(main)?
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch))
    {
        fail!(
            "new: {} checks out {branch}; work on it there",
            holder.top.display()
        );
    }
    Ok(())
}

/// An exclusive lock on <main>'s repo for picking a worktree (flock on a file
/// in the common git dir), held until the returned file is dropped.
fn lock_picking(main: &Path) -> Result<std::fs::File> {
    let path = main.join(".git/builds-new.lock");
    let file = crate::flock::open_creating(&path)?;
    crate::flock::lock(&file, libc::LOCK_EX, &path)?;
    Ok(file)
}

/// Adds the next side-N worktree of <main>'s repo, at <start>.
fn add_new(tool: &Tool, main: &Path, start: &Start) -> Result<std::path::PathBuf> {
    let repo = crate::checkout::file_name(main);
    let parent = main
        .parent()
        .expect("a main checkout has a folder")
        .join(format!("{repo}-worktrees"));
    let mut n = 1;
    while parent.join(format!("side-{n}")).exists()
        || tool
            .places
            .root
            .join(&repo)
            .join(format!("side-{n}"))
            .exists()
    {
        n += 1;
    }
    let dir = parent.join(format!("side-{n}"));
    std::fs::create_dir_all(&parent).context(|| format!("creating {}", parent.display()))?;
    // The hooks the common .git points at may belong to another checkout;
    // this one's own post-checkout runs in `prepare`, after its build paths
    // are linked.
    let mut add = git(main);
    add.args(["-c", "core.hooksPath=/dev/null", "worktree", "add", "-q"]);
    match start {
        Start::Fresh(Some(branch)) => add.arg("-b").arg(branch).arg(&dir).arg("origin/main"),
        Start::Fresh(None) => add.arg("--detach").arg(&dir).arg("origin/main"),
        Start::Resume(branch) => add.arg(&dir).arg(branch),
    };
    run_checked(&mut add, "git worktree add")?;
    Ok(dir)
}

/// Checks <start> out in the free worktree <free>: a new branch at
/// origin/main, HEAD detached there when the caller names no branch, or the
/// branch to resume. The checkout loses nothing: a free worktree is clean
/// and holds no commit missing from origin/main.
fn reuse(free: &Checkout, start: &Start) -> Result<()> {
    let mut checkout = git(&free.top);
    checkout.args(["-c", "core.hooksPath=/dev/null", "checkout", "-q"]);
    match start {
        Start::Fresh(Some(branch)) => checkout.arg("-b").arg(branch).arg("origin/main"),
        Start::Fresh(None) => checkout.arg("--detach").arg("origin/main"),
        Start::Resume(branch) => checkout.arg(branch).arg("--"),
    };
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
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// `builds retire <checkout>`: removes a clean, unclaimed side worktree and
/// its build root.
pub fn cmd_retire(tool: &Tool, args: &[String]) -> Result<()> {
    let [arg] = args else {
        fail!("usage: builds retire <checkout>")
    };
    let checkout = Checkout::find(arg)?;
    let top = checkout.top.display();
    if !checkout.is_side() {
        let what = if checkout.is_main() {
            "the main checkout"
        } else {
            "not one of the repo's side worktrees"
        };
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

/// Removes the unclaimed side worktree <checkout> that nothing keeps
/// (kept.rs), and its build root.
pub fn retire_checkout(tool: &Tool, checkout: &Checkout) -> Result<()> {
    let top = checkout.top.display();
    let root = checkout.build_root(&tool.places);
    if let Some(claim) = Claim::read(&checkout.claim_file())? {
        fail!(
            "{top} is claimed by {}; builds release it first",
            claim.text()
        );
    }
    let every = crate::lists::store::every_lists(&tool.places)?;
    let commits = crate::procs::git_commits(&crate::procs::Processes::read());
    let kept = crate::kept::read(&every, checkout, &commits)?;
    if let Some(parked) = kept.parked.first() {
        fail!(
            "{top} holds the work of {parked}, parked there; take the item there (`builds item take`) to work on it"
        );
    }
    kept.require_leavable("retire", &checkout.top)?;
    run_checked(
        git(&checkout.main)
            .arg("worktree")
            .arg("remove")
            .arg(&checkout.top),
        "git worktree remove",
    )?;
    if root.is_dir() && !evict(&tool.places, &root, Recreate::No, Removal::Background)? {
        fail!(
            "worktree removed, but a process holds files in {}; run builds prune once it exits",
            root.display()
        );
    }
    note(format!("retired {top} and deleted {}", root.display()));
    Ok(())
}

/// `builds app-checkout [remove] mac|android --host <host> <path>`: records
/// (or takes out) the app checkout `~/dev/<repo>-worktrees/<name>` on the host
/// <path> names, as the entry `<host>:<name>` of the lane's set (app_remote.rs).
/// The first build there makes the checkout.
fn app_checkout_on_host(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: builds app-checkout mac|android --host <host> <path> | builds app-checkout remove mac|android --host <host> <path>";
    let (removing, lane, host, path) = match args {
        [word, lane, flag, host, path] if word == "remove" && flag == "--host" => {
            (true, lane, host, path)
        }
        [lane, flag, host, path] if flag == "--host" => (false, lane, host, path),
        _ => fail!("{USAGE}"),
    };
    let Some(lane) = Lane::parse(lane) else {
        fail!("{USAGE}")
    };
    if !crate::host::read_remote(&tool.places)?
        .iter()
        .any(|known| known.name == *host)
    {
        fail!("no host named {host} (`builds host list`)");
    }
    let (repo, name) = crate::app_remote::name_in(path)?;
    tool.places.require_volume()?;
    let entry = crate::app_remote::entry_of(host, &name);
    let Some(remote) = RemoteApp::find(&tool.places, ssh, &repo, lane, &entry)? else {
        fail!("{entry} names no host");
    };
    if removing {
        // The lane file goes first: a host that does not answer fails the
        // removal and leaves the checkout in the lane, for the person to
        // remove again.
        remove_from_lane(tool, &repo, lane, &entry, &entry, || remote.unmark())?;
        note(format!(
            "removed {entry} from {repo}'s {} app checkouts, and its lane file on {host}, so {host} no longer keeps its builds warm; ~/dev/{repo}-worktrees/{name} on {host} stays until you remove it there",
            lane.word()
        ));
        return Ok(());
    }
    if let Some(other) = tool
        .places
        .app_lane_of(&repo, &entry)?
        .filter(|other| *other != lane)
    {
        fail!(
            "{entry} is {repo}'s {} app checkout already; the lanes exist so that their builds share no checkout",
            other.word()
        );
    }
    // The lane file comes first: a host that does not answer fails the
    // addition and leaves the lane as it was, for the person to add it again.
    remote.mark()?;
    let count = record_in_lane(tool, &repo, lane, &entry)?;
    note(format!(
        "{entry} is one of {repo}'s {} app checkouts ({count} in all, in {}); its first build makes ~/dev/{repo}-worktrees/{name} on {host} when {host} has none",
        lane.word(),
        tool.places.app_checkouts_file(&repo, lane).display()
    ));
    Ok(())
}

/// `builds app-checkout mac|android <checkout>`: adds the side worktree to its
/// repo's app checkouts for the lane; `builds app-checkout remove mac|android
/// <checkout>` takes it out again, while no build holds its lock; `builds
/// app-checkout reset <repo> <checkout>` puts one on origin's main (app_reset.rs).
pub fn cmd_app_checkout(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: builds app-checkout mac|android <checkout> | builds app-checkout mac|android --host <host> <path> | builds app-checkout remove mac|android [--host <host>] <checkout or path> | builds app-checkout reset <repo> <checkout> [--matching <rev>]";
    if args.iter().any(|arg| arg == "--host") {
        return app_checkout_on_host(tool, ssh, args);
    }
    let reset = match args {
        [word, repo, name] if word == "reset" => Some((repo, name, None)),
        [word, repo, name, flag, rev] if word == "reset" && flag == "--matching" => {
            Some((repo, name, Some(rev.as_str())))
        }
        _ => None,
    };
    if let Some((repo, name, matching)) = reset {
        tool.places.require_volume()?;
        return crate::app_reset::cmd_reset(&tool.places, ssh, repo, name, matching);
    }
    let (removing, lane, arg) = match args {
        [word, lane, arg] if word == "remove" => (true, lane, arg),
        [lane, arg] => (false, lane, arg),
        _ => fail!("{USAGE}"),
    };
    let Some(lane) = Lane::parse(lane) else {
        fail!("{USAGE}")
    };
    let checkout = Checkout::find(arg)?;
    if checkout.is_main() {
        fail!(
            "{} is the main checkout, which keeps its builds warm already; an app checkout is a side worktree",
            checkout.top.display()
        );
    }
    if !checkout.is_side() {
        fail!(
            "{} is not one of the repo's side worktrees; an app checkout is one",
            checkout.top.display()
        );
    }
    tool.places.require_volume()?;
    checkout.require_repo_name(&tool.places)?;
    let repo = checkout.repo();
    let name = checkout.name();
    if removing {
        let shown = checkout.top.display().to_string();
        remove_from_lane(tool, &repo, lane, &name, &shown, || Ok(()))?;
        note(format!(
            "removed {} from {repo}'s {} app checkouts",
            checkout.top.display(),
            lane.word()
        ));
        return Ok(());
    }
    if let Some(other) = tool
        .places
        .app_lane_of(&repo, &name)?
        .filter(|other| *other != lane)
    {
        fail!(
            "{} is {repo}'s {} app checkout already; the lanes exist so that their builds share no checkout",
            checkout.top.display(),
            other.word()
        );
    }
    let count = record_in_lane(tool, &repo, lane, &name)?;
    note(format!(
        "{} is one of {repo}'s {} app checkouts ({count} in all, in {}); the free-space floor keeps the folders its lane builds into",
        checkout.top.display(),
        lane.word(),
        tool.places.app_checkouts_file(&repo, lane).display()
    ));
    Ok(())
}

/// Records <entry> as one of <repo>'s <lane> app checkouts that a person
/// added, under the lane's queue guard. When the builds tool added the
/// checkout, the person now owns it, so it stays until a person removes it.
/// Returns how many checkouts the lane has.
fn record_in_lane(tool: &Tool, repo: &str, lane: Lane, entry: &str) -> Result<usize> {
    let queue = crate::queue::Queue::of(&tool.places, repo, lane);
    let guard = queue.guard()?;
    let mut set = tool.places.app_checkouts_recorded(repo, lane)?;
    match set.iter_mut().find(|checkout| checkout.entry == entry) {
        Some(checkout) => checkout.added_by = crate::lane::AddedBy::Person,
        None => set.push(crate::lane::AppCheckout::by_person(entry)),
    }
    tool.places.write_app_checkouts(repo, lane, &set)?;
    drop(guard);
    crate::queue::repoint(&tool.places, repo, lane);
    Ok(set.len())
}

/// Takes <entry> (shown to the person as <shown>) out of <repo>'s <lane> app
/// checkouts and deletes its lock, holding that lock so no build takes the
/// checkout meanwhile, and runs <before> under the lock first; a failure of
/// <before> leaves the checkout in the lane. Fails, before <before> runs,
/// when the lane has no such checkout or a build holds its lock.
fn remove_from_lane(
    tool: &Tool,
    repo: &str,
    lane: Lane,
    entry: &str,
    shown: &str,
    before: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if !tool
        .places
        .app_checkouts_of(repo, lane)?
        .iter()
        .any(|e| e == entry)
    {
        fail!(
            "{shown} is not one of {repo}'s {} app checkouts",
            lane.word()
        );
    }
    let Some(lock) = crate::lane_size::lock_checkout(&tool.places, repo, lane, entry)? else {
        fail!("a build holds {shown}'s lock; remove it once that build ends");
    };
    before()?;
    crate::lane_size::take_out(&tool.places, repo, lane, &lock, None)
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;

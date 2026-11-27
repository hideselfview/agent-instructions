//! Side worktrees: adding one, retiring one, and recording a repo's app
//! checkout.
//!
//! A side worktree builds host Rust only, and a repo with app builds keeps
//! them built in main and in its app checkouts, one or more worktrees per lane
//! (mac, android) that builds hands to no agent and whose names are not side
//! worktrees', which agents take turns in by holding a checkout's lock,
//! <root>/<repo>/app-<app>.locks/<checkout>; see APP BUILDS in help.txt. Nothing here enforces who builds what: the caps
//! bound whatever a checkout builds, and the floor bounds every checkout but
//! main and the folders an app checkout's lane builds into.

use std::path::Path;

use crate::app_remote::RemoteApp;
use crate::checkout::{APP_LANE_FILE, Checkout};
use crate::claim::{
    Claim, Note, require_one_line, require_project_named, set_project, write_claim,
};
use crate::evict::{Evicted, Recreate, Removal};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::free_worktrees::find_free;
use crate::host::Ssh;
use crate::lane::{AppCheckout, Lane, require_app_checkout_name};
use crate::output::note;
use crate::roots::delete;
use crate::setup::setup;
use crate::tool::Tool;
use boards::{git, git_text};

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
/// back into a worktree this way. When a step after the claim fails, `new`
/// undoes what it made (the claim, a worktree it added with its branch and
/// build root, or the checkout of a free one) and fails with that step's error.
pub fn cmd_new(tool: &Tool, ssh: &crate::host::Ssh, args: &[String]) -> Result<()> {
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
    let who = who.unwrap_or_else(|| "builds new".to_string());
    let handed = hand_out(tool, ssh, &repo_path, start, &who, project.as_deref())?;
    crate::output::answer_line(handed.checkout.top.display());
    Ok(())
}

/// A side worktree that `hand_out` claimed and set up, with what it needs to
/// undo that.
pub struct Handed {
    pub checkout: Checkout,
    start: Start,
    /// The worktree was added for this call, not a free one handed out again.
    added: bool,
}

impl Handed {
    /// Undoes `hand_out` after <error>, the failure of a later step: drops the
    /// claim, and removes the worktree it added or checks the free one out at
    /// origin/main again. Returns <error>, with the failure of the undo when
    /// that failed too. It holds the picking lock, so no `builds new` picks the
    /// worktree while it goes back.
    pub fn undo(self, tool: &Tool, error: Fail) -> Fail {
        let main = &self.checkout.main;
        let undone = lock_picking(main).and_then(|_picking| {
            if self.added {
                undo_add(tool, main, &self.checkout.top, &self.start)
            } else {
                undo_reuse(main, &self.checkout, &self.start, true)
            }
        });
        undo(error, undone)
    }
}

/// Claims a side worktree of the repo at <repo_path> for <who>, set up with
/// <start> checked out, and records <project> as the project of its work:
/// a free worktree (`find_free`) checked out afresh from origin/main, or the
/// next side-N when none is free. When a step after the claim fails, it
/// undoes what it made and returns that step's error.
pub(crate) fn hand_out(
    tool: &Tool,
    ssh: &crate::host::Ssh,
    repo_path: &str,
    start: Start,
    who: &str,
    project: Option<&str>,
) -> Result<Handed> {
    tool.places.require_volume()?;
    let main = Checkout::find(repo_path)?.main;
    if let Start::Resume(branch) = &start {
        require_resumable(&main, branch)?;
    }
    // Held from picking a worktree to claiming it, so two calls never get the
    // same one; released when the call ends.
    let _picking = lock_picking(&main)?;
    run_checked(git(&main).args(["fetch", "-q", "origin"]), "git fetch")?;
    let repo = Checkout::find(&main.display().to_string())?.repo();
    if let Some(project) = project
        && !crate::lists::store::projects_holding(&tool.places, &repo)?
            .iter()
            .any(|held| held == project)
    {
        fail!("new: project {project} holds no repo {repo}");
    }
    require_project_named(&tool.places, &repo, project)?;
    let (checkout, added) = match find_free(tool, &main)? {
        Some(free) => {
            let mut switched = false;
            let started = write_claim(&free, who, tool.now, Some(Note::Starting))
                .and_then(|()| set_project(&tool.places, &free, project))
                .and_then(|()| reuse(&free, &start))
                .and_then(|()| {
                    switched = true;
                    prepare(tool, ssh, &free)
                });
            if let Err(error) = started {
                return Err(undo(error, undo_reuse(&main, &free, &start, switched)));
            }
            (free, false)
        }
        None => {
            let dir = add_new(tool, &main, &start, side_name, &[])?;
            let started = Checkout::find(&dir.display().to_string()).and_then(|checkout| {
                write_claim(&checkout, who, tool.now, Some(Note::Starting))?;
                set_project(&tool.places, &checkout, project)?;
                prepare(tool, ssh, &checkout)?;
                Ok(checkout)
            });
            match started {
                Ok(checkout) => (checkout, true),
                Err(error) => {
                    return Err(undo(error, undo_add(tool, &main, &dir, &start)));
                }
            }
        }
    };
    Ok(Handed {
        checkout,
        start,
        added,
    })
}

/// <error>, the failure of a step of `builds new`, with the failure of the
/// <undone> cleanup that followed it, when that failed too.
fn undo(error: Fail, undone: Result<()>) -> Fail {
    match undone {
        Ok(()) => error,
        Err(cleanup) => Fail::Message(format!(
            "{}; and builds could not undo what `builds new` made: {}",
            error.text(),
            cleanup.text()
        )),
    }
}

/// Undoes `builds new` on the free worktree <free>: drops its claim and
/// project, and when <switched> (the checkout of <start> ran), detaches it at
/// origin/main again and deletes the branch the checkout made.
fn undo_reuse(main: &Path, free: &Checkout, start: &Start, switched: bool) -> Result<()> {
    crate::whole_file::remove_if_present(&free.claim_file())?;
    crate::whole_file::remove_if_present(&free.project_file())?;
    if !switched {
        return Ok(());
    }
    run_checked(
        git(&free.top).args([
            "-c",
            "core.hooksPath=/dev/null",
            "checkout",
            "-q",
            "--detach",
            "origin/main",
        ]),
        "git checkout",
    )?;
    delete_made_branch(main, start)
}

/// Deletes the branch `new` made for <start>: the one `-b` names, none for a
/// detached start or a resumed branch.
fn delete_made_branch(main: &Path, start: &Start) -> Result<()> {
    if let Start::Fresh(Some(branch)) = start {
        run_checked(
            git(main).args(["branch", "-q", "-D", branch]),
            "git branch -D",
        )?;
    }
    Ok(())
}

/// Undoes `builds new` on the worktree <dir> it added: removes the worktree,
/// the branch it made for it, and its build root.
fn undo_add(tool: &Tool, main: &Path, dir: &Path, start: &Start) -> Result<()> {
    let root = tool
        .places
        .root
        .join(crate::checkout::file_name(main))
        .join(crate::checkout::file_name(dir));
    crate::worktree_removal::remove(&Checkout::find(&dir.display().to_string())?)?;
    delete_made_branch(main, start)?;
    if root.is_dir() && delete(tool, &root, Recreate::No, Removal::Background)? == Evicted::InUse {
        fail!(
            "a process holds files in {}; run builds prune once it exits",
            root.display()
        );
    }
    Ok(())
}

/// What a handed-out worktree checks out.
pub(crate) enum Start {
    /// origin/main, on a new branch of this name or detached.
    Fresh(Option<String>),
    /// An existing branch no worktree checks out.
    Resume(String),
}

/// Refuses to resume <branch> in <main>'s repo when it does not exist or a
/// worktree checks it out: git would refuse the checkout after `builds new`
/// claimed the worktree.
fn require_resumable(main: &Path, branch: &str) -> Result<()> {
    if !crate::git::branch_exists(main, branch)? {
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

/// An exclusive lock on <main>'s repo for picking or removing a worktree
/// (flock on a file in the common git dir), held until the returned file is
/// dropped. Before it returns, it ends each removal an earlier process left
/// (worktree_removal.rs), so its holder reads no worktree removed in part.
pub(crate) fn lock_picking(main: &Path) -> Result<std::fs::File> {
    let path = main.join(".git/builds-new.lock");
    let file = crate::flock::open_creating(&path)?;
    crate::flock::lock(&file, libc::LOCK_EX, &path)?;
    crate::worktree_removal::finish(main)?;
    Ok(file)
}

/// The name of the <n>th side worktree.
fn side_name(n: usize) -> String {
    format!("side-{n}")
}

/// Adds a worktree of <main>'s repo at <start>, under the first name that
/// <name_of> gives for 1 and up that no worktree or build root has and
/// <taken> lacks.
fn add_new(
    tool: &Tool,
    main: &Path,
    start: &Start,
    name_of: impl Fn(usize) -> String,
    taken: &[String],
) -> Result<std::path::PathBuf> {
    let repo = crate::checkout::file_name(main);
    let parent = crate::checkout::worktrees_folder(main).expect("a main checkout has a folder");
    let name = (1..)
        .map(name_of)
        .find(|name| {
            !parent.join(name).exists()
                && !tool.places.root.join(&repo).join(name).exists()
                && !taken.contains(name)
        })
        .expect("some number names no worktree");
    let dir = parent.join(name);
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
fn prepare(tool: &Tool, ssh: &crate::host::Ssh, checkout: &Checkout) -> Result<()> {
    let dir = &checkout.top;
    setup(tool, ssh, checkout)?;
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

pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// `builds retire <checkout>`: removes an unclaimed side worktree that holds
/// nothing origin/main lacks, and its build root.
pub fn cmd_retire(tool: &Tool, args: &[String]) -> Result<()> {
    let [arg] = args else {
        fail!("{}", crate::command::Command::Retire.usage_text())
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
    let picking = lock_picking(&checkout.main)?;
    retire_checkout(tool, &checkout, &picking)
}

/// Removes the unclaimed side worktree <checkout> that nothing keeps and
/// whose HEAD origin/main covers (kept.rs), whole (worktree_removal.rs),
/// and its build root. <_picking> is the repo's picking lock, so no `builds
/// new` hands the worktree out meanwhile.
pub fn retire_checkout(tool: &Tool, checkout: &Checkout, _picking: &std::fs::File) -> Result<()> {
    let top = checkout.top.display();
    let root = checkout.build_root(&tool.places);
    if let Some(claim) = Claim::read(&checkout.claim_file())? {
        fail!(
            "{top} is claimed by {}; builds release it first",
            claim.text()
        );
    }
    let every = crate::lists::store::every_lists(&tool.places)?;
    let commits = crate::procs::git_commits(&crate::procs::Processes::read()?);
    let kept = crate::kept::read(&every, checkout, &commits)?;
    if let Some(keeping) = kept.keeping.first() {
        fail!(
            "{top} holds the work of {keeping}, which keeps it; take the item there (`builds item take`) to work on it"
        );
    }
    kept.require_removable("retire", &checkout.top)?;
    crate::worktree_removal::remove(checkout)?;
    if root.is_dir() && delete(tool, &root, Recreate::No, Removal::Background)? == Evicted::InUse {
        fail!(
            "worktree removed, but a process holds files in {}; run builds prune once it exits",
            root.display()
        );
    }
    note(format!("retired {top} and deleted {}", root.display()));
    Ok(())
}

/// `builds app-checkout [remove] <app> --host <host> <path>`: records
/// (or takes out) the app checkout `~/dev/<repo>-worktrees/<name>` on the host
/// <path> names, as the entry `<host>:<name>` of the lane's set (app_remote.rs).
/// The first build there makes the checkout.
fn app_checkout_on_host(tool: &Tool, ssh: &Ssh, args: &[String], removing: bool) -> Result<()> {
    let [lane, flag, host, path] = args else {
        fail!(
            "{}",
            crate::command::Command::group_usage_text("app-checkout")
        );
    };
    if flag != "--host" {
        fail!(
            "{}",
            crate::command::Command::group_usage_text("app-checkout")
        );
    }
    if !crate::host::read_remote(&tool.places)?
        .iter()
        .any(|known| known.name == *host)
    {
        fail!("no host named {host} (`builds host list`)");
    }
    let (repo, name) = crate::app_remote::name_in(path)?;
    let lane = crate::repo_lanes::parse(&tool.places, &repo, lane)?;
    tool.places.require_volume()?;
    let entry = crate::app_remote::entry_of(host, &name);
    if removing {
        // The lane file goes first: a host that does not answer fails the
        // removal and leaves the checkout in the lane, for the person to
        // remove again.
        remove_from_lane(tool, &repo, lane, &entry, &entry, |checkout| {
            RemoteApp::for_host(&tool.places, ssh, &repo, lane, host, &name, checkout)?.unmark()
        })?;
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
    let recorded = AppCheckout::by_person(&entry)?;
    // The lane file comes first: a host that does not answer fails the
    // addition and leaves the lane as it was, for the person to add it again.
    let marked = crate::app_remote::mark(&tool.places, ssh, &repo, lane, host, &name)?;
    let count = record_in_lane(tool, &repo, lane, recorded)?;
    let worktree = format!("~/dev/{repo}-worktrees/{name}");
    let there = if marked {
        format!(
            "builds wrote {APP_LANE_FILE} in {worktree}'s git dir on {host}, so {host} keeps its builds warm"
        )
    } else {
        format!("{host} has no {worktree} yet, and its first build makes it")
    };
    note(format!(
        "{entry} is one of {repo}'s {} app checkouts ({count} in all, in {}); {there}",
        lane.word(),
        tool.places.app_checkouts_file(&repo, lane).display()
    ));
    Ok(())
}

/// `builds app-checkout <app> <checkout>`: sets up the worktree <checkout>,
/// one in <repo>-worktrees that is no side worktree, as `builds new` sets one
/// up, and adds it to its repo's app checkouts for the lane.
pub fn cmd_app_checkout(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    add_or_remove(tool, ssh, args, false)
}

/// `builds app-checkout remove <app> <checkout>`: takes the checkout out of
/// its lane again, while no build holds its lock.
pub fn cmd_app_checkout_remove(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    add_or_remove(tool, ssh, args, true)
}

/// `builds app-checkout reset <repo> <checkout>`: puts an app checkout on
/// origin's main (app_reset.rs).
pub fn cmd_app_checkout_reset(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    let (repo, name, matching) = match args {
        [repo, name] => (repo, name, None),
        [repo, name, flag, rev] if flag == "--matching" => (repo, name, Some(rev.as_str())),
        _ => fail!(
            "{}",
            crate::command::Command::AppCheckout(crate::command::app_checkout::Reset).usage_text()
        ),
    };
    tool.places.require_volume()?;
    crate::app_reset::cmd_reset(&tool.places, ssh, repo, name, matching, &tool.changed)
}

/// Adds the app checkout <args> name to its lane, or removes it when
/// <removing>.
fn add_or_remove(tool: &Tool, ssh: &Ssh, args: &[String], removing: bool) -> Result<()> {
    if args.iter().any(|arg| arg == "--host") {
        return app_checkout_on_host(tool, ssh, args, removing);
    }
    let [lane, arg] = args else {
        fail!(
            "{}",
            crate::command::Command::group_usage_text("app-checkout")
        )
    };
    let checkout = Checkout::find(arg)?;
    if checkout.is_main() {
        fail!(
            "{} is the main checkout, which keeps its builds warm already; name another worktree",
            checkout.top.display()
        );
    }
    let Some(name) = checkout.name_in_worktrees() else {
        fail!(
            "{} is not in the repo's worktrees folder, <repo>-worktrees; an app checkout is a worktree there",
            checkout.top.display()
        );
    };
    tool.places.require_volume()?;
    checkout.require_repo_name(&tool.places)?;
    let repo = checkout.repo();
    let lane = crate::repo_lanes::parse_in_source(&checkout.top, lane)?;
    // The lane a root belongs to decides the cap of each folder the
    // dashboard shows.
    tool.changed.add(checkout.build_root(&tool.places));
    if removing {
        remove_local_checkout(tool, &repo, lane, &name, &checkout)
    } else {
        add_local_checkout(tool, ssh, &repo, lane, &name, &checkout)
    }
}

/// Takes <checkout>, the worktree <name>, out of <repo>'s <lane>.
fn remove_local_checkout(
    tool: &Tool,
    repo: &str,
    lane: Lane,
    name: &str,
    checkout: &Checkout,
) -> Result<()> {
    let shown = checkout.top.display().to_string();
    remove_from_lane(tool, repo, lane, name, &shown, |_| Ok(()))?;
    note(format!(
        "removed {shown} from {repo}'s {} app checkouts; the worktree stays until you remove it with `git worktree remove`, and `builds prune` then deletes its build folders",
        lane.word()
    ));
    Ok(())
}

/// Sets up <checkout>, the worktree <name>, unless <repo>'s <lane> holds it
/// already, and records it in the lane.
fn add_local_checkout(
    tool: &Tool,
    ssh: &Ssh,
    repo: &str,
    lane: Lane,
    name: &str,
    checkout: &Checkout,
) -> Result<()> {
    // A folder's name may hold a colon, which an entry reads as a host's.
    require_app_checkout_name(name)?;
    let recorded = AppCheckout::by_person(name)?;
    if let Some(other) = tool
        .places
        .app_lane_of(repo, name)?
        .filter(|other| *other != lane)
    {
        fail!(
            "{} is {repo}'s {} app checkout already; the lanes exist so that their builds share no checkout",
            checkout.top.display(),
            other.word()
        );
    }
    if !tool
        .places
        .app_checkouts_of(repo, lane)?
        .iter()
        .any(|entry| entry == name)
    {
        prepare(tool, ssh, checkout)?;
        start_from_warmest(tool, ssh, repo, lane, checkout)?;
    }
    let count = record_in_lane(tool, repo, lane, recorded)?;
    note(format!(
        "{} is one of {repo}'s {} app checkouts ({count} in all, in {}); the free-space floor keeps the folders its lane builds into",
        checkout.top.display(),
        lane.word(),
        tool.places.app_checkouts_file(repo, lane).display()
    ));
    Ok(())
}

/// Fills the empty cargo build folders of <checkout>, a worktree about
/// to become an app checkout of <repo>'s <lane>, from the lane's warmest
/// checkout on this Mac, held under its lock (app_clone.rs). With no such
/// checkout, the first build is cold.
fn start_from_warmest(
    tool: &Tool,
    ssh: &Ssh,
    repo: &str,
    lane: Lane,
    checkout: &Checkout,
) -> Result<()> {
    let at = crate::lane_size::RepoLane {
        places: &tool.places,
        ssh,
        repo,
        lane,
    };
    let Some(taken) = crate::app_clone::take_source(at, crate::host::LOCAL, &checkout.name())?
    else {
        note(format!(
            "{repo}'s {} lane has no warm checkout on this Mac to clone into {}; its first build is cold",
            lane.word(),
            checkout.top.display()
        ));
        return Ok(());
    };
    let top = crate::app_config::app_checkout_top(&tool.places, repo, &taken.entry)?;
    crate::warm::clone_lane(
        tool,
        lane,
        &Checkout::find(&top.display().to_string())?,
        checkout,
    )
}

/// Gives <repo>'s <lane> an app checkout on this Mac when it has none, for a
/// build that runs only here (an `.app-build` `here` step, or the Android
/// install): a new worktree of the repo's clone at ~/dev/<repo> that builds
/// names (`lane_size::builds_name`), detached at origin/main, set up as
/// `builds new` sets one up and recorded in the lane as a checkout of the
/// person's, which builds never removes. A lane that has one already is left
/// as it is.
pub fn ensure_local_app_checkout(tool: &Tool, ssh: &Ssh, repo: &str, lane: Lane) -> Result<()> {
    let has_local = || -> Result<bool> {
        Ok(tool
            .places
            .app_checkouts_of(repo, lane)?
            .iter()
            .any(|entry| crate::app_remote::split(entry).is_none()))
    };
    if has_local()? {
        return Ok(());
    }
    tool.places.require_volume()?;
    let main = tool.places.home.join(crate::host::clone_path(repo));
    if !main.is_dir() {
        fail!(
            "{repo} has no app checkout on this Mac, and {} is no clone to add one to",
            main.display()
        );
    }
    // Held from picking a name to recording it, so no other build adds one
    // meanwhile.
    let _picking = lock_picking(&main)?;
    if has_local()? {
        return Ok(());
    }
    run_checked(git(&main).args(["fetch", "-q", "origin"]), "git fetch")?;
    let start = Start::Fresh(None);
    let name_of = |n| crate::lane_size::builds_name(lane, n);
    // A lane's set may name a worktree that is gone, and its name stays taken.
    let taken = tool.places.app_checkout_entries(repo)?;
    let dir = add_new(tool, &main, &start, name_of, &taken)?;
    let made = Checkout::find(&dir.display().to_string()).and_then(|checkout| {
        prepare(tool, ssh, &checkout)?;
        Ok(checkout)
    });
    let recorded = made.and_then(|checkout| {
        let recorded = AppCheckout::by_person(&checkout.name())?;
        record_with_message(tool, repo, lane, recorded, || {
            crate::queue::repoint(&tool.places, repo, lane)
        })?;
        Ok(checkout)
    });
    match recorded {
        Ok(checkout) => {
            note(format!(
                "{repo}'s {} lane had no app checkout on this Mac, so builds made {} for it",
                lane.word(),
                checkout.top.display()
            ));
            Ok(())
        }
        Err(error) => Err(undo(error, undo_add(tool, &main, &dir, &start))),
    }
}

/// Records <checkout> as one of <repo>'s <lane> app checkouts that a person
/// added, under the lane's queue guard. When the builds tool added the
/// checkout, the person now owns it, so it stays until a person removes it.
/// Returns how many checkouts the lane has.
fn record_in_lane(tool: &Tool, repo: &str, lane: Lane, checkout: AppCheckout) -> Result<usize> {
    let count = write_in_lane(tool, repo, lane, checkout)?;
    crate::queue::repoint(&tool.places, repo, lane)?;
    Ok(count)
}

/// `record_in_lane` up to the lane's set: writes <added> into it, under the
/// lane's queue guard, and returns how many checkouts the lane has. Nothing
/// is written when it fails.
fn write_in_lane(tool: &Tool, repo: &str, lane: Lane, added: AppCheckout) -> Result<usize> {
    let queue = crate::queue::Queue::of(&tool.places, repo, lane);
    let guard = queue.guard()?;
    let mut set = tool.places.app_checkouts_recorded(repo, lane)?;
    match set
        .iter_mut()
        .find(|checkout| checkout.entry() == added.entry())
    {
        Some(checkout) => *checkout = added,
        None => set.push(added),
    }
    tool.places.write_app_checkouts(repo, lane, &set)?;
    drop(guard);
    Ok(set.len())
}

/// Writes <checkout> into <repo>'s <lane> set, then runs <tell>, the message
/// to the lane's queue to read the set again. The lane holds the checkout once
/// the write lands, so a failure of <tell> takes it out again, and a failed
/// write leaves the lane as it was.
fn record_with_message(
    tool: &Tool,
    repo: &str,
    lane: Lane,
    checkout: AppCheckout,
    tell: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let entry = checkout.entry().to_string();
    write_in_lane(tool, repo, lane, checkout)?;
    tell().map_err(|error| undo(error, forget_in_lane(tool, repo, lane, &entry)))
}

/// Takes <entry> out of <repo>'s <lane> set when it is in it, under the
/// lane's queue guard.
fn forget_in_lane(tool: &Tool, repo: &str, lane: Lane, entry: &str) -> Result<()> {
    let queue = crate::queue::Queue::of(&tool.places, repo, lane);
    let guard = queue.guard()?;
    let mut set = tool.places.app_checkouts_recorded(repo, lane)?;
    set.retain(|checkout| checkout.entry() != entry);
    tool.places.write_app_checkouts(repo, lane, &set)?;
    drop(guard);
    Ok(())
}

/// Takes <entry> (shown to the person as <shown>) out of <repo>'s <lane> app
/// checkouts and deletes its lock, holding that lock so no build takes the
/// checkout meanwhile, and runs <before> under the lock first; a failure of
/// <before> leaves the checkout in the lane. Fails, before <before> runs,
/// when a build holds its lock or the lane has no such checkout
/// (`lane_size::lock_checkout`).
fn remove_from_lane(
    tool: &Tool,
    repo: &str,
    lane: Lane,
    entry: &str,
    shown: &str,
    before: impl FnOnce(crate::app_lock_guard::CheckoutLock<'_>) -> Result<()>,
) -> Result<()> {
    let Some(lock) = crate::lane_size::lock_checkout(&tool.places, repo, lane, entry)? else {
        fail!("a build holds {shown}'s lock; remove it once that build ends");
    };
    before(lock.reservation())?;
    crate::lane_size::take_out(&tool.places, repo, lane, &lock, None)
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;

//! The temporary folder of a run, on the build volume.
//!
//! Every program builds starts for a build, a test, a shots run or an app
//! build writes its scratch files where TMPDIR points: `std::env::temp_dir()`,
//! the `tempfile` crate, the profile folder of the Chrome that the shots
//! checker starts, Gradle and xcodebuild. Left alone, TMPDIR is the system's
//! temporary folder, on a volume whose file changes fseventsd logs and spends
//! CPU on. The build volume has no such log (`builds help`, HOSTS), so each
//! run of a command that starts build work (`Command::makes_run_folder`)
//! makes `<build volume>/.tmp/<run>`, points its own TMPDIR at it before any thread starts, and every process it
//! starts inherits that. The run deletes the folder when it ends. Health
//! deletes the folders of runs whose process is gone (`sweep_host`).
//!
//! A run inside another run (an app build's lane, a `builds cargo` that a
//! step starts) finds TMPDIR already naming a run's folder and makes none:
//! the outermost run owns the folder.
//!
//! A job on another host runs through `remote::run_script`, which makes the
//! host's folder the same way. A folder is named for its run's id, which
//! `remote::job_id` makes from the pid of the `builds` process on this Mac
//! that owns the run and the time it started, so `Holders::holder` judges
//! every folder on every host from this Mac's process table.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::command::Command;
use crate::fail;
use crate::fail::{Context, Result, gone};
use crate::host::{Host, Ssh};
use crate::job_claim::{Holder, Holders};
use crate::places::Places;

/// The folder of every run's folder, in the build volume's root. A name that
/// starts with a dot is one of the volume's own (`Places::is_reserved_name`),
/// so no repo and no build root is read from it.
pub const FOLDER: &str = ".tmp";

#[derive(Debug, PartialEq, Eq)]
pub enum Folder {
    Run,
    Directory,
    NotDirectory,
    Missing,
}

/// Inspect a temporary folder that a process already uses. Its path can
/// reach a run's folder or depend on an intermediate directory in one.
pub fn classify(root: &Path, folder: &Path) -> io::Result<Folder> {
    let resolved = match folder.canonicalize() {
        Ok(folder) => folder,
        Err(error) if gone(&error) => return Ok(Folder::Missing),
        Err(error) => return Err(error),
    };
    match resolved.metadata() {
        Ok(metadata) if !metadata.is_dir() => return Ok(Folder::NotDirectory),
        Ok(_) => {}
        Err(error) if gone(&error) => return Ok(Folder::Missing),
        Err(error) => return Err(error),
    }
    let parent = match root.join(FOLDER).canonicalize() {
        Ok(parent) => parent,
        Err(error) if gone(&error) => return Ok(Folder::Directory),
        Err(error) => return Err(error),
    };
    if under_runs(&parent, &resolved) {
        return Ok(Folder::Run);
    }
    if folder != resolved && passes_through_run(&parent, folder)? {
        return Ok(Folder::Run);
    }
    Ok(Folder::Directory)
}

/// Canonical paths show the destination. The original path and the paths
/// named by its links also show which directories it needs to get there.
fn passes_through_run(parent: &Path, folder: &Path) -> io::Result<bool> {
    let mut pending = vec![folder.to_path_buf()];
    let mut inspected = HashSet::new();
    while let Some(path) = pending.pop() {
        let path: PathBuf = path.components().collect();
        for step in path.ancestors() {
            if !inspected.insert(step.to_path_buf()) {
                break;
            }
            if step != folder && under_runs(parent, &step.canonicalize()?) {
                return Ok(true);
            }
            match step.read_link() {
                Ok(link) => pending.push(step.with_file_name(link)),
                Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(false)
}

fn under_runs(parent: &Path, folder: &Path) -> bool {
    folder != parent && folder.starts_with(parent)
}

/// A run's folder, deleted when the value is dropped.
#[derive(Debug)]
pub struct RunTmp {
    folder: PathBuf,
}

impl Drop for RunTmp {
    fn drop(&mut self) {
        // A folder this cannot delete is a run's leftover: the process is
        // gone by then, so the next health pass deletes it (`sweep_here`).
        if let Err(error) = std::fs::remove_dir_all(&self.folder) {
            crate::output::note(format!(
                "deleting the run's temporary folder {}: {error}",
                self.folder.display()
            ));
        }
    }
}

/// Makes the folder of the run that <command> starts and sets this process's
/// TMPDIR to it, when the command starts build work
/// (`Command::makes_run_folder`) and TMPDIR
/// does not already name an existing run's folder. Fails when the build
/// volume is not mounted: a run that wrote to the system's temporary folder
/// instead would write where fseventsd logs it.
///
/// Call it before the process starts a thread: it changes the environment.
pub fn enter(places: &Places, command: Command) -> Result<Option<RunTmp>> {
    if !command.makes_run_folder() {
        return Ok(None);
    }
    places.require_volume()?;
    for (name, value) in crate::sccache::client_env() {
        // SAFETY: the caller has started no other thread.
        unsafe { std::env::set_var(name, value) };
    }
    let parent = places.root.join(FOLDER);
    if places.tmpdir.as_ref().is_some_and(|tmp| {
        tmp.is_absolute()
            && under_runs(&parent, tmp)
            && !tmp
                .components()
                .any(|part| part == std::path::Component::ParentDir)
            && tmp.is_dir()
    }) {
        return Ok(None);
    }
    let folder = parent.join(crate::remote::job_id());
    std::fs::create_dir_all(&folder).context(|| format!("creating {}", folder.display()))?;
    // SAFETY: the caller has started no other thread, so none reads the
    // environment while set_var writes it.
    unsafe { std::env::set_var("TMPDIR", &folder) };
    Ok(Some(RunTmp { folder }))
}

/// The shell lines that make the folder of the job whose script is <run>'s,
/// on a host reached over ssh, and set TMPDIR to it. They follow
/// `remote::root_script`, which sets BUILDS_ROOT. On this Mac they are empty:
/// the job inherits the TMPDIR of the `builds` process that starts it.
pub fn remote_setup(host: &Host, run: &str) -> String {
    match host.target {
        Some(_) => {
            let cache_env: String = crate::sccache::client_env()
                .into_iter()
                .map(|(name, value)| format!("export {name}={}\n", crate::host::sh_quote(&value)))
                .collect();
            format!(
                r#"tmp="$BUILDS_ROOT/{FOLDER}/{run}"
mkdir -p "$tmp" || exit 97
TMPDIR="$tmp"
export TMPDIR
{cache_env}"#
            )
        }
        None => String::new(),
    }
}

/// The shell line that deletes the folder `remote_setup` made.
pub fn remote_cleanup(host: &Host) -> &'static str {
    match host.target {
        Some(_) => "rm -rf \"$tmp\"\n",
        None => "",
    }
}

/// What a sweep of one host's run folders did.
#[derive(Debug, Default, PartialEq)]
pub struct Swept {
    /// The folders it deleted: the run that made each one ended.
    pub deleted: Vec<String>,
    /// The entries of the host's run folders that no run of builds names.
    pub unexpected: Vec<String>,
}

impl Swept {
    fn judge(names: Vec<String>, holders: &Holders) -> (Vec<String>, Swept) {
        let mut gone = Vec::new();
        let mut swept = Swept::default();
        for name in names {
            match holders.holder(&name) {
                Some(Holder::Gone) => gone.push(name),
                Some(Holder::Runs) => {}
                None => swept.unexpected.push(name),
            }
        }
        (gone, swept)
    }
}

/// Deletes each run folder on <host> whose run has ended (`Holders::holder`),
/// and lists the entries that are no run's folder. Fails when the host cannot
/// be listed or a folder cannot be deleted.
pub fn sweep(places: &Places, ssh: &Ssh, host: &Host, holders: &Holders) -> Result<Swept> {
    let parent = places.root.join(FOLDER);
    if host.target.is_none() {
        let entries = match std::fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Swept::default());
            }
            Err(error) => fail!("listing {}: {error}", parent.display()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.context(|| format!("listing {}", parent.display()))?;
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        let (gone, mut swept) = Swept::judge(names, holders);
        for name in gone {
            let path = parent.join(&name);
            std::fs::remove_dir_all(&path).context(|| format!("deleting {}", path.display()))?;
            swept.deleted.push(path.display().to_string());
        }
        return Ok(swept);
    }
    let transport = crate::remote::Transport { places, ssh, host };
    let root = crate::remote::root_script(host);
    let listed = transport.capture(
        &format!("{root}d=\"$BUILDS_ROOT/{FOLDER}\"\n[ -d \"$d\" ] || exit 0\nls -A \"$d\""),
        "listing the temporary folders of runs",
    )?;
    let (gone, mut swept) = Swept::judge(listed.lines().map(str::to_string).collect(), holders);
    if !gone.is_empty() {
        // A name `holder` judged is digits and one dash: safe in a script.
        let removals: String = gone
            .iter()
            .map(|name| format!("rm -rf \"$BUILDS_ROOT/{FOLDER}/{name}\" || exit 1\n"))
            .collect();
        transport.capture(
            &format!("{root}{removals}"),
            "deleting the temporary folders of ended runs",
        )?;
        swept
            .deleted
            .extend(gone.iter().map(|name| format!("{FOLDER}/{name}")));
    }
    Ok(swept)
}

impl crate::command::Command {
    /// Whether the command makes a run's temporary folder (run_tmp.rs): it
    /// starts build work.
    pub fn makes_run_folder(self) -> bool {
        use crate::command::Command::*;
        use crate::command::*;
        match self {
            Cargo
            | Lake
            | AppBuild(
                app_build::Build
                | app_build::LandingCoverage
                | app_build::DropShots
                | app_build::RecordHanded,
            )
            | BuildApp
            | Lock
            | Remote(remote::Run | remote::Step)
            | Shots(shots::Checker)
            | Web(web::Picture) => true,
            Help
            | Ack
            | RulesOnEdit
            | Setup
            | Tool
            | New
            | Claim
            | Note
            | Release
            | Retire
            | Discard
            | Programs
            | Queue
            | Takeover
            | AppClone
            | SccacheTmpdir
            | Prune
            | Du
            | Status
            | Health
            | InstallHook
            | Waits
            | Land
            | Brief
            | Install
            | Version
            | Next
            | Agent(agent::Start | agent::Run | agent::Resume | agent::List | agent::Show)
            | Shots(shots::Findings | shots::Picture | shots::CheckProgram)
            | AppCheckout(app_checkout::Add | app_checkout::Remove | app_checkout::Reset)
            | Host(
                host::Add
                | host::Setup
                | host::SetupHere
                | host::SpeedHere
                | host::Band
                | host::List
                | host::Cache
                | host::Remove
                | host::ClearXcode,
            )
            | Units(units::Prune | units::GitCheckouts | units::AfterBuild)
            | Dashboard(dashboard::Stage | dashboard::Serve)
            | Checks(checks::Run | checks::Whole | checks::Stats | checks::List | checks::Tests)
            | Review(
                review::Run
                | review::Copy
                | review::Settle
                | review::Wait
                | review::Dispute
                | review::Keep
                | review::Show
                | review::Key
                | review::CheckLanding
                | review::Landed
                | review::Groups
                | review::Replay
                | review::Audit
                | review::Misses
                | review::Rates
                | review::Scores
                | review::CasePropose
                | review::CaseAdd,
            )
            | Judge(judge::Listen)
            | Question(
                question::Ask
                | question::Answer
                | question::Settle
                | question::Done
                | question::Drop
                | question::Place
                | question::Edit
                | question::Show
                | question::List,
            )
            | Item(
                item::Add
                | item::Edit
                | item::Note
                | item::Changes
                | item::Wait
                | item::Backlog
                | item::Start
                | item::Take
                | item::CheckBrief
                | item::Handoff
                | item::Stop
                | item::Finished
                | item::Drop
                | item::Place
                | item::CheckLanding
                | item::Landed
                | item::Show
                | item::List
                | item::Times,
            )
            | Area(area::Add | area::Rename | area::Move | area::Remove)
            | Decision(
                decision::Add | decision::Edit | decision::Find | decision::Show | decision::List,
            )
            | Design(
                design::Places
                | design::Only
                | design::Record
                | design::Amend
                | design::Approve
                | design::Check
                | design::Landed
                | design::Title
                | design::Drop
                | design::Show
                | design::List
                | design::Render
                | design::TokensCss
                | design::PublishFind
                | design::PublishFiles
                | design::PublishLast
                | design::Listen
                | design::PublishTake
                | design::PublishDone
                | design::PublishFailed,
            )
            | Lists(
                lists::Init | lists::DesignGlobs | lists::AddRepo | lists::Show | lists::ProjectOf,
            ) => false,
        }
    }
}

#[cfg(test)]
#[path = "run_tmp_tests.rs"]
mod tests;

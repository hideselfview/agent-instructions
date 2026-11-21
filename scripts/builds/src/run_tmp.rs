//! The temporary folder of a run, on the build volume.
//!
//! Every program builds starts for a build, a test, a shots run or an app
//! build writes its scratch files where TMPDIR points: `std::env::temp_dir()`,
//! the `tempfile` crate, the profile folder of the Chrome that the shots
//! checker starts, Gradle and xcodebuild. Left alone, TMPDIR is the system's
//! temporary folder, on a volume whose file changes fseventsd logs and spends
//! CPU on. The build volume has no such log (`builds help`, HOSTS), so each
//! run of a command
//! that starts build work (`starts_work`) makes `<build volume>/.tmp/<run>`,
//! points its own TMPDIR at it before any thread starts, and every process it
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

use std::path::PathBuf;

use crate::fail;
use crate::fail::{Context, Result};
use crate::host::{Host, Ssh};
use crate::job_claim::{Holder, Holders};
use crate::places::Places;

/// The folder of every run's folder, in the build volume's root. A name that
/// starts with a dot is one of the volume's own (`Places::is_reserved_name`),
/// so no repo and no build root is read from it.
pub const FOLDER: &str = ".tmp";

/// The commands that start build work, whose runs get a folder: the runs of
/// cargo and Lean, the app builds and the lock they run under, the jobs on
/// other hosts, and the shots checker (every `builds shots` subcommand but the
/// two that read the cached runs).
pub fn starts_work(command: &str, args: &[String]) -> bool {
    match command {
        "cargo" | "lake" | "app-build" | "build-app" | "lock" | "remote" => true,
        "shots" => crate::shots_program::runs_checker(args),
        _ => false,
    }
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
            crate::text::note(format!(
                "deleting the run's temporary folder {}: {error}",
                self.folder.display()
            ));
        }
    }
}

/// Makes the folder of the run that `builds <command> <args>` starts and sets
/// this process's TMPDIR to it, when the command starts build work and TMPDIR
/// does not already name an existing run's folder. Fails when the build
/// volume is not mounted: a run that wrote to the system's temporary folder
/// instead would write where fseventsd logs it.
///
/// Call it before the process starts a thread: it changes the environment.
pub fn enter(places: &Places, command: &str, args: &[String]) -> Result<Option<RunTmp>> {
    if !starts_work(command, args) {
        return Ok(None);
    }
    places.require_volume()?;
    let parent = places.root.join(FOLDER);
    if places
        .tmpdir
        .as_ref()
        .is_some_and(|tmp| tmp.starts_with(&parent) && tmp.is_dir())
    {
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
        Some(_) => format!(
            r#"tmp="$BUILDS_ROOT/{FOLDER}/{run}"
mkdir -p "$tmp" || exit 97
TMPDIR="$tmp"
export TMPDIR
"#,
        ),
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

#[cfg(test)]
#[path = "run_tmp_tests.rs"]
mod tests;

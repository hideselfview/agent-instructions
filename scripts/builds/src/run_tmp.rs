//! The temporary folder of a run, on the build volume.
//!
//! Every program builds starts for a build, a test, a shots run or an app
//! build writes its scratch files where TMPDIR points: `std::env::temp_dir()`,
//! the `tempfile` crate, the profile folder of the Chrome that the shots
//! checker starts, Gradle and xcodebuild. Left alone, TMPDIR is the system's
//! temporary folder, on a volume whose file changes fseventsd logs, and the
//! writes of one board render kept fseventsd at half a core. The build
//! volume has no such log (`builds help`, HOSTS), so each run of a command
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
//! host's folder the same way, named for the script's own pid (`$$`), so the
//! folder's name starts with the pid of the process that owns it on either
//! machine.

use std::path::PathBuf;

use crate::fail;
use crate::fail::{Context, Result};
use crate::host::{Host, Ssh};
use crate::places::Places;

/// The folder of every run's folder, in the build volume's root. A name that
/// starts with a dot is one of the volume's own (`Places::is_reserved_name`),
/// so no repo and no build root is read from it.
pub const FOLDER: &str = ".tmp";

/// The name of the folder of the run <id> that the process <pid> owns. <pid>
/// is a number here, and `$$` in the script of a host (`remote_setup`), so
/// the part before the first dash is always the owner's pid (`owner`).
pub fn name(pid: &str, id: &str) -> String {
    format!("{pid}-{id}")
}

/// The pid that owns the folder named <name> (`name`), or None for a name
/// builds did not make.
pub fn owner(name: &str) -> Option<i32> {
    let (pid, _) = name.split_once('-')?;
    pid.parse().ok()
}

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
    let folder = parent.join(name(
        &std::process::id().to_string(),
        &crate::job_claim::now_nanos().to_string(),
    ));
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
            r#"tmp="$BUILDS_ROOT/{FOLDER}/{name}"
mkdir -p "$tmp" || exit 97
TMPDIR="$tmp"
export TMPDIR
"#,
            name = name("$$", run),
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

/// Deletes each folder in <places>' run folders whose owner no longer runs
/// (`procs::runs`), and returns the paths it deleted.
pub fn sweep_here(places: &Places) -> Result<Vec<PathBuf>> {
    let parent = places.root.join(FOLDER);
    let entries = match std::fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", parent.display()),
    };
    let mut deleted = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", parent.display()))?;
        let Some(pid) = owner(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        if crate::procs::runs(pid) {
            continue;
        }
        let path = entry.path();
        std::fs::remove_dir_all(&path).context(|| format!("deleting {}", path.display()))?;
        deleted.push(path);
    }
    Ok(deleted)
}

/// `sweep_here` on <host>: the paths of the folders it deleted, one for each
/// line. On a host over ssh the owner is alive when `kill -0` finds its pid.
pub fn sweep_host(places: &Places, ssh: &Ssh, host: &Host) -> Result<Vec<String>> {
    if host.target.is_none() {
        return Ok(sweep_here(places)?
            .iter()
            .map(|path| path.display().to_string())
            .collect());
    }
    let script = format!(
        r#"{root}d="$BUILDS_ROOT/{FOLDER}"
[ -d "$d" ] || exit 0
for f in "$d"/*; do
  [ -e "$f" ] || continue
  pid=${{f##*/}}
  pid=${{pid%%-*}}
  case $pid in ''|*[!0-9]*) continue ;; esac
  kill -0 "$pid" 2>/dev/null && continue
  rm -rf "$f" && echo "$f"
done"#,
        root = crate::remote::root_script(host),
    );
    let transport = crate::remote::Transport { places, ssh, host };
    let said = transport.capture(&script, "deleting the temporary folders of ended runs")?;
    Ok(said.lines().map(str::to_string).collect())
}

#[cfg(test)]
#[path = "run_tmp_tests.rs"]
mod tests;

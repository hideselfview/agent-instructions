//! The programs an app build runs: the builds and builds-shots it started
//! with, from start to end, whatever an install puts on PATH meanwhile.
//!
//! Each version of builds keeps its two programs in a folder of its own,
//! `~/.local/lib/builds/<version>/`, named by `builds version`'s hash, which
//! covers builds-shots' sources too (build.rs). An app build copies the
//! programs it started from into its version's folder when the folder lacks
//! them, after it checks that each copy says this version (an install that
//! replaced a program after this build started puts another version there).
//! It holds a shared lock on the folder's `.in-use` for its life, starts its
//! lanes from the folder's builds, and puts the folder first on PATH, so every
//! `builds` its steps run, `builds shots run` among them, and the
//! builds-shots that runs, are this version's. A lane's steps on a host run
//! the same way: the lane copies the folder to the host's
//! `~/.local/lib/builds/<version>/` when the host lacks it, and runs each step
//! under `<folder>/builds programs hold`, which holds the host's folder and
//! puts it first on PATH (app_remote.rs).
//!
//! `builds install`, here and on each host, removes each other version's
//! folder that nothing holds and that nothing used in the last `IDLE`: each
//! pin and each copy to a host marks the folder used.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::fail;
use crate::fail::{Context, Result};
use crate::flock::{lock, open_creating, try_lock};

/// The folder of the versions' folders, relative to the home folder.
pub const FOLDER: &str = ".local/lib/builds";

/// The file in a version's folder that each build using it holds a shared
/// lock on, and whose time says when a build last used it.
pub const IN_USE: &str = ".in-use";

/// How long a version's folder stays after a build last used it, whether or
/// not a build holds it: longer than the gap between a lane's copy to a host
/// and its step's hold there.
pub const IDLE: Duration = Duration::from_secs(60 * 60);

/// The folder of the versions' folders in <home>.
pub fn root(home: &Path) -> PathBuf {
    home.join(FOLDER)
}

/// The folder of <version>'s programs in <home>.
pub fn folder(home: &Path, version: &str) -> PathBuf {
    root(home).join(version)
}

/// The programs a build started from, and the versions they must say.
pub struct Sources<'a> {
    pub builds: &'a Path,
    pub shots: &'a Path,
    /// What `builds version` says after `builds `.
    pub version: &'a str,
    /// What `builds-shots version` says.
    pub shots_version: &'a str,
}

/// A version's folder that this process holds.
pub struct Pinned {
    dir: PathBuf,
    in_use: File,
}

impl Pinned {
    /// The folder's builds.
    pub fn builds(&self) -> PathBuf {
        self.dir.join("builds")
    }

    /// <path>, a PATH, with the folder first.
    pub fn path(&self, path: Option<&OsStr>) -> OsString {
        let mut joined = OsString::from(&self.dir);
        if let Some(rest) = path.filter(|rest| !rest.is_empty()) {
            joined.push(":");
            joined.push(rest);
        }
        joined
    }

    /// The lock's file, set to stay open across exec, and <path> with the
    /// folder first: what `builds programs hold` keeps and sets before it
    /// runs its command in its place.
    pub fn for_exec(self, path: Option<&OsStr>) -> Result<(File, OsString)> {
        use std::os::fd::AsRawFd;
        let joined = self.path(path);
        let fd = self.in_use.as_raw_fd();
        // SAFETY: fcntl on a descriptor this value owns; no memory is touched.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
            fail!(
                "keeping {} open for the command: {}",
                self.dir.join(IN_USE).display(),
                std::io::Error::last_os_error()
            );
        }
        Ok((self.in_use, joined))
    }
}

/// What `pin` found.
pub enum Pin {
    Pinned(Pinned),
    /// A program where this build started from says another version: an
    /// install replaced it after this build started. Why, for the caller,
    /// which runs the installed builds in its place.
    Replaced(String),
}

/// Whether the path <path> names the open file <file>.
fn names(file: &File, path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let open = file
        .metadata()
        .context(|| format!("reading {}", path.display()))?;
    match std::fs::metadata(path) {
        Ok(named) => Ok(named.dev() == open.dev() && named.ino() == open.ino()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}

/// What <program> says when it is run with `version`.
fn says(program: &Path) -> Result<String> {
    let ran = std::process::Command::new(program)
        .arg("version")
        .stdin(std::process::Stdio::null())
        .output()
        .context(|| format!("running {} version", program.display()))?;
    Ok(String::from_utf8_lossy(&ran.stdout).trim().to_string())
}

/// Holds the folder of <sources>' version in <home>, and copies each program
/// the folder lacks from <sources> first. Returns `Replaced`, and copies
/// nothing more, when a program at <sources> says another version.
pub fn pin(home: &Path, sources: &Sources) -> Result<Pin> {
    let dir = folder(home, sources.version);
    let in_use_path = dir.join(IN_USE);
    let in_use = loop {
        std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
        let file = match open_creating(&in_use_path) {
            Ok(file) => file,
            // An install removed the folder after this build made it.
            Err(_) if !dir.is_dir() => continue,
            Err(error) => return Err(error),
        };
        lock(&file, libc::LOCK_SH, &in_use_path)?;
        if names(&file, &in_use_path)? {
            break file;
        }
    };
    in_use
        .set_modified(SystemTime::now())
        .context(|| format!("marking {} used", dir.display()))?;
    let wanted = [
        (
            "builds",
            sources.builds,
            format!("builds {}", sources.version),
        ),
        (
            crate::shots_program::PROGRAM,
            sources.shots,
            sources.shots_version.to_string(),
        ),
    ];
    for (name, source, version) in wanted {
        let into = dir.join(name);
        if into.is_file() {
            continue;
        }
        let staged = dir.join(format!(".{name}.{}", crate::text::staged_suffix()));
        std::fs::copy(source, &staged)
            .context(|| format!("copying {} to {}", source.display(), staged.display()))?;
        let said = says(&staged);
        if said.as_deref().ok() != Some(version.as_str()) {
            let _ = std::fs::remove_file(&staged);
            let said = said.unwrap_or_else(|error| error.text());
            return Ok(Pin::Replaced(format!(
                "{} says {said:?}, not {version:?}: builds install replaced it after this build started",
                source.display()
            )));
        }
        std::fs::rename(&staged, &into)
            .context(|| format!("moving {} to {}", staged.display(), into.display()))?;
    }
    Ok(Pin::Pinned(Pinned { dir, in_use }))
}

/// `pin` with this program and the builds-shots beside it, of this
/// program's version.
pub fn pin_this(home: &Path) -> Result<Pin> {
    let builds = std::env::current_exe().context(|| "finding this program".to_string())?;
    let shots = crate::shots_program::beside(&builds);
    pin(
        home,
        &Sources {
            builds: &builds,
            shots: &shots,
            version: env!("BUILDS_SOURCE"),
            shots_version: crate::shots_program::VERSION,
        },
    )
}

/// When a build last used the folder <dir>: the time of its `IN_USE`, or of
/// the folder itself while a copy has made the folder and not yet the file.
fn used_at(dir: &Path) -> Result<SystemTime> {
    let file = dir.join(IN_USE);
    let read = match std::fs::metadata(&file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::metadata(dir),
        other => other,
    };
    read.and_then(|meta| meta.modified())
        .context(|| format!("reading the time of {}", dir.display()))
}

/// Removes each version's folder in <home> but <keep>'s that no build holds
/// and that no build used in the `IDLE` before <now>; returns the versions
/// it removed, in the order of their names.
pub fn remove_unused(home: &Path, keep: &str, now: SystemTime) -> Result<Vec<String>> {
    let root = root(home);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", root.display()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", root.display()))?;
        if entry.path().is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    let mut removed = Vec::new();
    for name in names {
        if name == keep {
            continue;
        }
        let dir = root.join(&name);
        let in_use = dir.join(IN_USE);
        // Held while the folder goes: a build that opened the file waits,
        // then finds the path names no file and makes the folder again.
        let _alone = match File::open(&in_use) {
            Ok(file) => {
                if !try_lock(&file, libc::LOCK_EX, &in_use)? {
                    continue;
                }
                Some(file)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => fail!("opening {}: {error}", in_use.display()),
        };
        if now.duration_since(used_at(&dir)?).unwrap_or_default() < IDLE {
            continue;
        }
        std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
        removed.push(name);
    }
    Ok(removed)
}

/// `remove_unused` for this program's version, now, saying each version it
/// removed: `builds install` here and `builds host setup-here` on each host.
pub fn remove_others(home: &Path) -> Result<()> {
    for version in remove_unused(home, env!("BUILDS_SOURCE"), SystemTime::now())? {
        crate::text::note(format!(
            "removed {}, which no app build uses",
            folder(home, &version).display()
        ));
    }
    Ok(())
}

const USAGE: &str = "usage: builds programs hold -- <command> [<args>...]";

/// `builds programs hold -- <command>...`: holds this program's version's
/// folder, puts it first on PATH, and runs the command in this process's
/// place, which keeps the hold until it and every process it starts that
/// keeps the lock's file open end. A lane runs each step on a host this way.
pub fn cmd_programs(home: &Path, args: &[String]) -> Result<()> {
    let [hold, dashes, command @ ..] = args else {
        fail!("{USAGE}");
    };
    if hold != "hold" || dashes != "--" || command.is_empty() {
        fail!("{USAGE}");
    }
    let pinned = match pin_this(home)? {
        Pin::Pinned(pinned) => pinned,
        Pin::Replaced(why) => fail!("{why}"),
    };
    let (_kept, path) = pinned.for_exec(std::env::var_os("PATH").as_deref())?;
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(&command[0])
        .args(&command[1..])
        .env("PATH", path)
        .exec();
    fail!("running {}: {error}", command[0])
}

#[cfg(test)]
#[path = "programs_tests.rs"]
mod tests;

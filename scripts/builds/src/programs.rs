//! The program an app build runs: the builds it started with, from start to
//! end, whatever an install puts on PATH meanwhile.
//!
//! Each version of builds keeps its program in a folder of its own,
//! `~/.local/lib/builds/<version>/`, named by `builds version`'s hash, which
//! covers the shots checker's sources too (build.rs). An app build copies the
//! builds it started from into its version's folder when the folder lacks
//! it, after it checks that the copy says this version (an install that
//! replaced the program after this build started puts another version
//! there). When the folder holds a builds already, the build compares its
//! bytes with the builds it started from and stops when they differ, even
//! though both say this version. It holds a shared lock on the folder's `.in-use` for its life,
//! starts its lanes from the folder's builds, and puts the folder first on
//! PATH, so every `builds` its steps run, `builds shots run` among them, is
//! this version's. A lane's steps on a host run
//! the same way: the lane copies the folder to the host's
//! `~/.local/lib/builds/<version>/` when the host lacks it, and runs each step
//! under `<folder>/builds programs hold`, which holds the host's folder and
//! puts it first on PATH (app_remote.rs).
//!
//! `builds install` removes each other version's folder here that no build
//! holds, then has each host remove each version's folder there that it does
//! not name: it names the folders left here, which a build here holds or
//! the installed builds is, so no host loses a folder a build here still
//! runs steps from.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};
use crate::flock::{lock, names, open_creating, try_lock};

/// The folder of the versions' folders, relative to the home folder.
pub const FOLDER: &str = ".local/lib/builds";

/// The file in a version's folder that each build using it holds a shared
/// lock on.
pub const IN_USE: &str = ".in-use";

/// The folder of the versions' folders in <home>.
pub fn root(home: &Path) -> PathBuf {
    home.join(FOLDER)
}

/// The folder of <version>'s builds in <home>.
pub fn folder(home: &Path, version: &str) -> PathBuf {
    root(home).join(version)
}

/// A version's folder that this process holds.
pub struct Pinned {
    version: String,
    dir: PathBuf,
    in_use: File,
}

impl Pinned {
    /// The version, as `builds version` says it after `builds `.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

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
    /// The program this build started from says another version: an
    /// install replaced it after this build started. Why, for the caller,
    /// which runs the installed builds in its place.
    Replaced(String),
}

/// What <program> says when it is run with `version`; fails when it cannot
/// run or exits with an error.
fn says(program: &Path) -> Result<String> {
    let ran = std::process::Command::new(program)
        .arg("version")
        .stdin(std::process::Stdio::null())
        .output()
        .context(|| format!("running {} version", program.display()))?;
    if !ran.status.success() {
        fail!(
            "{} version failed ({}): {}",
            program.display(),
            ran.status,
            String::from_utf8_lossy(&ran.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&ran.stdout).trim().to_string())
}

/// Why <builds>, which says <said>, is not the program of <wanted>.
fn replaced(builds: &Path, said: &str, wanted: &str) -> String {
    format!(
        "{} says {said:?}, not {wanted:?}: builds install replaced it after this build started",
        builds.display()
    )
}

/// Whether the files at <a> and <b> hold the same bytes.
fn same_bytes(a: &Path, b: &Path) -> Result<bool> {
    let read = |path: &Path| std::fs::read(path).context(|| format!("reading {}", path.display()));
    Ok(read(a)? == read(b)?)
}

/// Holds the folder of <version> in <home>, and copies <builds>, the builds
/// this build started from, into it first when the folder lacks it. Returns
/// `Replaced`, and copies nothing, when <builds> says another version than
/// `builds <version>`. Fails when the folder holds a builds that differs
/// from <builds> in its bytes, though both say <version>.
pub fn pin(home: &Path, builds: &Path, version: &str) -> Result<Pin> {
    let dir = folder(home, version);
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
    let into = dir.join("builds");
    let wanted = format!("builds {version}");
    if into.is_file() {
        // The folder holds a program of this version already. The program
        // this build started from must be that program, byte for byte: two
        // programs can say one version and differ, and the build would run
        // the folder's, not the one it started from.
        match says(builds) {
            Ok(said) if said == wanted => {}
            Ok(said) => return Ok(Pin::Replaced(replaced(builds, &said, &wanted))),
            Err(error) => return Err(error),
        }
        if !same_bytes(builds, &into)? {
            fail!(
                "{} and {} both say {wanted:?} and differ: the folder holds another program of this version than the one this build started from, so the version does not name the program. Remove {} with no build running, or install again",
                builds.display(),
                into.display(),
                dir.display()
            );
        }
    } else {
        let staged = dir.join(format!(".builds.{}", crate::text::staged_suffix()));
        std::fs::copy(builds, &staged)
            .context(|| format!("copying {} to {}", builds.display(), staged.display()))?;
        let said = match says(&staged) {
            Ok(said) if said == wanted => None,
            Ok(said) => Some(Ok(said)),
            Err(error) => Some(Err(error)),
        };
        if let Some(said) = said {
            let removed =
                std::fs::remove_file(&staged).context(|| format!("removing {}", staged.display()));
            let said = crate::fail::with_cleanup(said, removed)?;
            return Ok(Pin::Replaced(replaced(builds, &said, &wanted)));
        }
        std::fs::rename(&staged, &into)
            .context(|| format!("moving {} to {}", staged.display(), into.display()))?;
    }
    Ok(Pin::Pinned(Pinned {
        version: version.to_string(),
        dir,
        in_use,
    }))
}

/// `pin` with this program, of this program's version.
pub fn pin_this(home: &Path) -> Result<Pin> {
    pin(home, &crate::this_program()?, env!("BUILDS_SOURCE"))
}

/// The versions whose folders <home> holds, in the order of their names.
pub fn versions(home: &Path) -> Result<Vec<String>> {
    let root = root(home);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", root.display()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", root.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() && !name.starts_with('.') {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Removes each version's folder in <home> that <keep> does not name and
/// that no build holds; returns the versions it removed, in the order of
/// their names.
pub fn remove_unused(home: &Path, keep: &[&str]) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for name in versions(home)? {
        if keep.contains(&name.as_str()) {
            continue;
        }
        let dir = folder(home, &name);
        let in_use = dir.join(IN_USE);
        // Held while the folder goes: a build that opened the file waits,
        // then finds that the path names no file and makes the folder again.
        let _alone = match File::open(&in_use) {
            Ok(file) => {
                if !try_lock(&file, libc::LOCK_EX, &in_use)? {
                    continue;
                }
                Some(file)
            }
            // A build that made the folder and not yet the file finds the
            // folder gone when it opens the file, and makes it again.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => fail!("opening {}: {error}", in_use.display()),
        };
        std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
        removed.push(name);
    }
    Ok(removed)
}

/// `remove_unused` keeping this program's version and <also>, saying each
/// version it removed: `builds install` here, with nothing in <also>, and
/// `builds host setup-here` on each host, with the versions left on the Mac
/// that installs it.
pub fn remove_others(home: &Path, also: &[String]) -> Result<()> {
    let mut keep = vec![env!("BUILDS_SOURCE")];
    keep.extend(also.iter().map(String::as_str));
    for version in remove_unused(home, &keep)? {
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

//! The store: one folder per project under `Places::lists`, holding
//! `lists.json` (the whole state) and `lists.lock`. Every write takes the
//! lock exclusively, reads the state, applies the command, runs the
//! whole-state check, and replaces `lists.json` by an atomic rename, so a
//! write lands whole or not at all and a refused command changes nothing.
//! Readers take no lock: the rename is atomic.
//!
//! Nothing here reads or writes outside the project's folder, except the
//! listing of the parent folder that finds which project a repo is in.

use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::args::Args;
use crate::lists::check::{check, project_name_fault};
use crate::lists::{Lists, LogLine, Stamp};
use crate::places::Places;

const STATE: &str = "lists.json";
const LOCK: &str = "lists.lock";

/// One project's folder.
#[derive(Clone, Debug)]
pub struct Store {
    pub project: String,
    pub dir: PathBuf,
}

/// An exclusive lock on the store, held until dropped.
struct Locked {
    _file: File,
}

impl Locked {
    fn take(path: &Path) -> Result<Locked> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .context(|| format!("opening {}", path.display()))?;
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(Locked { _file: file });
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                fail!("locking {}: {error}", path.display());
            }
        }
    }
}

impl Store {
    /// The store of <project>; the name must be one a folder can have.
    pub fn named(places: &Places, project: &str) -> Result<Store> {
        if let Some(fault) = project_name_fault(project) {
            fail!("{fault}");
        }
        Ok(Store { project: project.to_string(), dir: places.lists.join(project) })
    }

    /// The store a command runs against: `--project`, else the project the
    /// repo of <cwd> belongs to.
    pub fn resolve(places: &Places, args: &Args, cwd: &Path) -> Result<Store> {
        if let Some(project) = args.value("project")? {
            return Store::named(places, project);
        }
        let checkout = match Checkout::at(cwd)? {
            Some(checkout) => checkout,
            None => fail!("{} is in no git checkout; say which project with --project", cwd.display()),
        };
        match project_of(places, &checkout)? {
            Some(project) => Store::named(places, &project),
            None => fail!("repo {} is in no project; say which with --project", checkout.repo()),
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.dir.join(STATE)
    }

    fn lock_path(&self) -> PathBuf {
        self.dir.join(LOCK)
    }

    /// Is there a store here?
    pub fn exists(&self) -> bool {
        self.state_path().is_file()
    }

    /// The state, checked. Takes no lock.
    pub fn load(&self) -> Result<Lists> {
        let path = self.state_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fail!(
                "there are no lists for project {} ({} is missing); `builds lists init {}` makes them",
                self.project,
                path.display(),
                self.project
            ),
            Err(error) => fail!("reading {}: {error}", path.display()),
        };
        let lists: Lists = serde_json::from_str(&text)
            .map_err(|error| Fail::Message(format!("{} is not valid lists: {error}", path.display())))?;
        self.verify(&lists)?;
        Ok(lists)
    }

    fn verify(&self, lists: &Lists) -> Result<()> {
        if lists.project != self.project {
            fail!("{} holds the lists of project {}, not {}", self.state_path().display(), lists.project, self.project);
        }
        check(lists)
    }

    /// Makes the store with <lists> as its first state: the state is checked,
    /// its version advances, and the log gains the line for <words>. Refused
    /// when the store exists.
    pub fn create(&self, mut lists: Lists, stamp: &Stamp, words: &str) -> Result<()> {
        advance(&mut lists, stamp, words);
        self.verify(&lists)?;
        std::fs::create_dir_all(&self.dir).context(|| format!("creating {}", self.dir.display()))?;
        let _lock = Locked::take(&self.lock_path())?;
        if self.exists() {
            fail!(
                "the lists of project {} already exist ({}); init makes them once",
                self.project,
                self.state_path().display()
            );
        }
        self.replace(&lists)
    }

    /// Applies <apply> to the state under the lock and writes the result,
    /// after the whole-state check, as one atomic replacement; the log gains
    /// a line for <words> by <stamp>. When <apply> or the check refuses,
    /// nothing is written and the file is byte for byte as it was.
    pub fn with_write<T>(&self, stamp: &Stamp, words: &str, apply: impl FnOnce(&mut Lists) -> Result<T>) -> Result<T> {
        if !self.dir.is_dir() {
            fail!(
                "there are no lists for project {} ({} is missing); `builds lists init {}` makes them",
                self.project,
                self.dir.display(),
                self.project
            );
        }
        let _lock = Locked::take(&self.lock_path())?;
        let mut lists = self.load()?;
        let result = apply(&mut lists)?;
        advance(&mut lists, stamp, words);
        self.verify(&lists)?;
        self.replace(&lists)?;
        Ok(result)
    }

    /// Moves the stored state from an older shape to the current one:
    /// <migrate> rewrites the state as raw JSON under the lock and says
    /// whether it changed anything. A change is read as the current shape,
    /// checked, and written as one more version with a log line for <words>,
    /// all or nothing; no change writes nothing.
    pub fn migrate(
        &self,
        stamp: &Stamp,
        words: &str,
        migrate: impl FnOnce(&mut serde_json::Value) -> Result<bool>,
    ) -> Result<bool> {
        let _lock = Locked::take(&self.lock_path())?;
        let path = self.state_path();
        let text = std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
        let mut raw: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| Fail::Message(format!("{} is not valid JSON: {error}", path.display())))?;
        if !migrate(&mut raw)? {
            return Ok(false);
        }
        let mut lists: Lists = serde_json::from_value(raw)
            .map_err(|error| Fail::Message(format!("{} migrated is not valid lists: {error}", path.display())))?;
        advance(&mut lists, stamp, words);
        self.verify(&lists)?;
        self.replace(&lists)?;
        Ok(true)
    }

    /// Writes <lists> to a temporary file beside the state, syncs it, renames
    /// it over the state and syncs the folder.
    fn replace(&self, lists: &Lists) -> Result<()> {
        let mut text = serde_json::to_string_pretty(lists).context(|| "writing the lists".to_string())?;
        text.push('\n');
        let temporary = self.dir.join(format!("{STATE}.tmp"));
        let mut file = File::create(&temporary).context(|| format!("creating {}", temporary.display()))?;
        file.write_all(text.as_bytes()).context(|| format!("writing {}", temporary.display()))?;
        file.sync_all().context(|| format!("syncing {}", temporary.display()))?;
        drop(file);
        std::fs::rename(&temporary, self.state_path()).context(|| format!("replacing {}", self.state_path().display()))?;
        File::open(&self.dir)
            .and_then(|dir| dir.sync_all())
            .context(|| format!("syncing {}", self.dir.display()))
    }
}

/// One more version, and its line in the log.
pub(crate) fn advance(lists: &mut Lists, stamp: &Stamp, words: &str) {
    lists.version += 1;
    lists.log.push(LogLine { version: lists.version, at: stamp.at, by: stamp.by.clone(), words: words.to_string() });
}

/// Sets the version, whatever the log says.
#[cfg(test)]
pub fn plant_version(lists: &mut Lists, version: u64) {
    lists.version = version;
}

/// Adds <line> to the log, whatever version it carries.
#[cfg(test)]
pub fn plant_log_line(lists: &mut Lists, line: LogLine) {
    lists.log.push(line);
}

/// The stores that exist under `Places::lists`, by project name.
pub fn stores(places: &Places) -> Result<Vec<Store>> {
    let entries = match std::fs::read_dir(&places.lists) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", places.lists.display()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", places.lists.display()))?;
        if entry.path().join(STATE).is_file() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names.iter().map(|name| Store::named(places, name)).collect()
}

/// One project's store as read: its lists, or what is wrong with them.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub project: String,
    pub lists: std::result::Result<Lists, String>,
}

/// Every store, each loaded and checked, for what reads them all (the
/// daemon, `builds health`): a store that fails stays in the answer with its
/// fault, since nothing repairs it. Err when the folder can't be listed.
pub fn load_all(places: &Places) -> std::result::Result<Vec<Loaded>, String> {
    let stores = stores(places).map_err(Fail::text)?;
    Ok(stores
        .into_iter()
        .map(|store| Loaded { lists: store.load().map_err(Fail::text), project: store.project })
        .collect())
}

/// The projects whose repos include <repo>, in name order.
pub fn projects_holding(places: &Places, repo: &str) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for store in stores(places)? {
        if store.load()?.repos.iter().any(|held| held == repo) {
            found.push(store.project);
        }
    }
    Ok(found)
}

/// The project <checkout>'s repo belongs to, or None. A repo in two projects
/// is refused.
pub fn project_of(places: &Places, checkout: &Checkout) -> Result<Option<String>> {
    let repo = checkout.repo();
    let mut found = projects_holding(places, &repo)?;
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        _ => fail!("repo {repo} is in two projects ({}); say which with --project", found.join(", ")),
    }
}

/// Who writes: `--by` when given, else the claim of the worktree <cwd> is in.
/// Neither is refused.
pub fn resolve_by(args: &Args, cwd: &Path) -> Result<String> {
    if let Some(by) = args.value("by")? {
        if by.trim().is_empty() {
            fail!("--by is empty");
        }
        return Ok(by.to_string());
    }
    if let Some(checkout) = Checkout::at(cwd)?
        && let Some(claim) = Claim::read(&checkout.claim_file())?
        && !claim.who.is_empty()
    {
        return Ok(claim.who);
    }
    fail!("say who you are with --by (this directory is in no claimed worktree)")
}

impl Stamp {
    /// The stamp of a write by <args>' `--by` or the claim at <cwd>, at <now>.
    pub fn resolve(now: i64, args: &Args, cwd: &Path) -> Result<Stamp> {
        Ok(Stamp { at: now, by: resolve_by(args, cwd)? })
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

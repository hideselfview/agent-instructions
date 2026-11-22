//! The landings of one repo that wait for a lane's landing turn, and the
//! batch the landing that gets the turn builds and pushes for all of them
//! (land.rs, `take_app_turn`).
//!
//! A landing that changes the apps of a lane first registers in the queue of
//! its home lane, the first lane its commits change in the order of
//! `Lane::ALL`, a folder in the repo's git folder, and then waits for the
//! home lane's turn. Registering writes an entry file that names the
//! landing's worktree, commits and branch, and the landing keeps an exclusive
//! lock on the file for as long as it runs, so a file nobody locks belongs to
//! a landing that ended. The landing that gets the turn leads the batch: it
//! reads the entries of the queue that nobody has settled, builds all their
//! commits on origin/main once, pushes every branch that passes and writes
//! each landing a verdict file. A landing that waited gets the turn once the
//! leader lets it go, finds its verdict and ends with it, while a landing
//! that the leader did not read leads the next batch.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;

/// The folder in a repo's git folder where the landings that wait for the
/// turn of <lane> register.
pub fn queue_dir(common: &Path, lane: Lane) -> PathBuf {
    common.join(format!("land-batch-{}", lane.word()))
}

/// What a landing tells the leader of its batch.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub worktree: String,
    pub shas: Vec<String>,
    /// The branch or commit the worktree had when the landing began, the
    /// name the branch's app builds were asked to build.
    pub rev: String,
    /// What the landing calls it, for its refusals.
    pub start: String,
    /// The lanes whose apps the commits change.
    pub lanes: Vec<Lane>,
}

/// How the leader of a batch settled one landing.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Verdict {
    /// The commits are on origin/main: their copies there, oldest first, and
    /// what the landing prints of them.
    Landed { on_main: Vec<String>, said: String },
    /// Nothing of the landing reached main. <output> is what the failing
    /// build printed, which the leader's own landing has already shown, and
    /// <said> what the landing prints.
    Refused { output: String, said: String },
}

/// A landing's place in a queue, which it holds until it ends.
pub struct Registered {
    dir: PathBuf,
    pub id: String,
    _lock: std::fs::File,
}

static REGISTERED: AtomicU32 = AtomicU32::new(0);

fn entry_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.entry"))
}

fn verdict_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.verdict"))
}

/// Writes <text> to <path> so that a reader sees all of it or none.
fn write_whole(path: &Path, text: &str) -> Result<()> {
    let temp = path.with_extension("new");
    std::fs::write(&temp, text).context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, path).context(|| format!("renaming {}", temp.display()))
}

/// Registers <entry> in the queue <dir>, which it makes when it is missing.
/// The entry's file is locked from the moment it has its name, and its text
/// is whole by then.
pub fn register(dir: &Path, entry: &Entry) -> Result<Registered> {
    std::fs::create_dir_all(dir).context(|| format!("making {}", dir.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context(|| "reading the clock".to_string())?
        .as_nanos();
    let id = format!(
        "{now:038}-{}-{}",
        std::process::id(),
        REGISTERED.fetch_add(1, Ordering::Relaxed)
    );
    let path = entry_path(dir, &id);
    let temp = path.with_extension("new");
    let file = crate::flock::open_creating(&temp)?;
    if !crate::flock::try_lock(&file, libc::LOCK_EX, &temp)? {
        fail!("{} is locked by another landing", temp.display());
    }
    let text = serde_json::to_string(entry).context(|| "writing the entry".to_string())?;
    // The write keeps the locked file: it fills the temporary file and renames it.
    write_whole(&path, &text)?;
    Ok(Registered {
        dir: dir.to_path_buf(),
        id,
        _lock: file,
    })
}

impl Registered {
    /// What the leader of an earlier batch settled for this landing, when it
    /// settled one.
    pub fn verdict(&self) -> Result<Option<Verdict>> {
        read_verdict(&self.dir, &self.id)
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(entry_path(&self.dir, &self.id));
        let _ = std::fs::remove_file(verdict_path(&self.dir, &self.id));
    }
}

fn read_verdict(dir: &Path, id: &str) -> Result<Option<Verdict>> {
    let path = verdict_path(dir, id);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("reading {}: {error}", path.display()),
    };
    match serde_json::from_str(&text) {
        Ok(verdict) => Ok(Some(verdict)),
        Err(error) => fail!("{} holds no verdict: {error}", path.display()),
    }
}

/// Writes the verdict of the landing <id> of the queue <dir>.
pub fn write_verdict(dir: &Path, id: &str, verdict: &Verdict) -> Result<()> {
    let text = serde_json::to_string(verdict).context(|| "writing the verdict".to_string())?;
    write_whole(&verdict_path(dir, id), &text)
}

/// The landings of the queue <dir> that run and wait for a verdict, oldest
/// first, as (id, entry). It removes the files of landings that ended, which
/// no process locks.
pub fn waiting(dir: &Path) -> Result<Vec<(String, Entry)>> {
    let names = match std::fs::read_dir(dir) {
        Ok(names) => names,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("reading {}: {error}", dir.display()),
    };
    let mut ids = Vec::new();
    let mut verdicts = Vec::new();
    for name in names {
        let name = name.context(|| format!("reading {}", dir.display()))?;
        let name = name.file_name().to_string_lossy().into_owned();
        if let Some(id) = name.strip_suffix(".entry") {
            ids.push(id.to_string());
        } else if let Some(id) = name.strip_suffix(".verdict") {
            verdicts.push(id.to_string());
        }
    }
    ids.sort();
    let mut found = Vec::new();
    for id in &ids {
        let path = entry_path(dir, id);
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => fail!("opening {}: {error}", path.display()),
        };
        if crate::flock::try_lock(&file, libc::LOCK_EX, &path)? {
            // Nobody holds the lock: the landing that wrote it ended.
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(verdict_path(dir, id));
            continue;
        }
        if read_verdict(dir, id)?.is_some() {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
        let entry = serde_json::from_str(&text).map_err(|error| {
            crate::fail::Fail::Message(format!("{} holds no entry: {error}", path.display()))
        })?;
        found.push((id.clone(), entry));
    }
    for id in verdicts.iter().filter(|id| !ids.contains(id)) {
        let _ = std::fs::remove_file(verdict_path(dir, id));
    }
    Ok(found)
}

#[cfg(test)]
#[path = "land_batch_tests.rs"]
mod tests;

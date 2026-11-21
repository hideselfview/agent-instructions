//! Where builds keeps the output of each `--shots` and `--all-shots` run: on
//! the build volume, never in a project's lists store. A run is build output,
//! kept so the next agent that wants the run of a commit, or the before side
//! of a branch's run, reads it instead of building.
//!
//! Each run has one folder, found by what made it:
//!
//!   <volume>/shots/<repo>/runs/<commit>/<lane>/<prefix>/<version>/
//!     run.json     when the run ended and the shots command's exit status
//!     output.txt   what the shots program printed, findings and verdict
//!     report/      the program's report as it wrote it: index.html, the
//!                  pictures that page shows and findings.json
//!
//! <commit> is the full commit id the run built, <prefix> the shot-name
//! prefix it was given (`mac.` for a lane's every shot), and <version> the
//! version of the shots program that made it. A later run of the same key
//! replaces the folder whole: a run fills a staging folder beside it and
//! moves it into place only when it is complete, so a reader finds either
//! the old run or the new one.
//!
//! Beside the runs, each repo has a picture folder,
//! `<volume>/shots/<repo>/pictures/`, which app-build passes to the shots
//! program (`run --pictures`): the program keeps each board picture it draws
//! there as `boards/<board hash>.png` and `.json`, the hash of everything the
//! picture depends on, and draws only the boards whose hash the folder lacks;
//! and it keeps each shot the app takes as
//! `<platform>/<app source tree>/<shot>.png` and `.json` (app_tree.rs), and
//! takes no shot again when the folder keeps every one the run selects.
//!
//! builds keeps each repo's folder in `shots` under `CAP_KIB`: `setup` and
//! `prune` delete the repo's whole runs, board pictures and app source
//! trees' shots, least recently used first, while it is over (prune.rs calls
//! `prune`). A run's last use is the time of its run.json, which the run
//! sets as it ends and each read sets again; a board picture's is the time of
//! its picture, and an app source tree's shots' the newest time in their
//! folder, which the shots program sets each time it takes them; so what
//! agents keep using stays.
//!
//! The folder `shots` is a capped store (capped_store.rs): its lock on
//! `shots/.writing` is held by each run that stores its output, and by
//! app-build while a shots step reads a before run or the picture folder or
//! writes to it, so the prune deletes nothing a run reads or writes.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::app_timing::now;
use crate::capped_store::{self, Group, Kept, Use};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::prune::Run;
use crate::text::note;
use crate::tool::Tool;

/// The folder on the build volume that holds every repo's runs.
pub const FOLDER: &str = "shots";

/// How much each repo's folder in `shots` may hold, its runs and its picture
/// folder together. A run of a lane's every shot keeps about 10 MB of
/// report, so the cap holds main's runs and many branch runs.
pub const CAP_KIB: u64 = 2 * 1024 * 1024;

/// The file of a run's folder that says how it ended.
const RUN_FILE: &str = "run.json";
/// The file of a run's folder that holds what the shots program printed.
const OUTPUT_FILE: &str = "output.txt";
/// The folder of a run's folder that holds its report.
const REPORT: &str = "report";
/// The folder of a picture folder that holds the board pictures.
pub const BOARD_PICTURES: &str = "boards";
/// The folder of an app source tree's shots where the shots program marks
/// each prefix it took every shot of.
const TAKEN: &str = ".taken";
/// What the name of a folder starts with that holds, in a picture folder,
/// the pictures a run on a host made while builds moves them into place.
const INCOMING: &str = ".incoming-";

/// What a stored run is found by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub repo: String,
    /// The full commit id.
    pub commit: String,
    pub lane: Lane,
    pub prefix: String,
    pub version: String,
}

/// What run.json holds.
#[derive(Serialize, Deserialize)]
struct Ending {
    /// When the run ended, seconds since 1970.
    recorded_at: i64,
    /// The shots command's exit status: 0 when no pair failed the run.
    exit_code: i64,
}

/// A stored run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub key: Key,
    pub recorded_at: i64,
    pub exit_code: i64,
    pub findings: String,
    /// The run's folder.
    pub folder: PathBuf,
}

impl Stored {
    /// The folder that holds the run's report.
    pub fn report(&self) -> PathBuf {
        self.folder.join(REPORT)
    }

    /// The run's findings as records, as the shots program wrote them beside
    /// its report; the file is missing when the run's report held none.
    pub fn findings_json(&self) -> PathBuf {
        self.report().join(crate::shot_causes::FILE)
    }
}

/// The folder `shots` on the volume of <places>.
pub fn root(places: &Places) -> PathBuf {
    places.root.join(FOLDER)
}

/// Fails unless <prefix> can be one folder's name.
fn check_prefix(prefix: &str) -> Result<()> {
    if prefix.is_empty() || prefix.contains('/') || prefix.starts_with('.') {
        fail!("{prefix:?} is no shot-name prefix: a prefix starts with a platform and holds no /");
    }
    Ok(())
}

/// The picture folder of <repo>, which the shots program reads and adds
/// board pictures to.
pub fn pictures(places: &Places, repo: &str) -> PathBuf {
    root(places).join(repo).join("pictures")
}

/// Whether <repo>'s picture folder keeps every shot a run of <prefix> takes
/// for <lane>'s app source tree <tree>: the shots program marked taken, for
/// that tree, a prefix that starts <prefix>, by a run that took the shots
/// shown in a window when <windows> asks for them.
pub fn shots_kept(
    places: &Places,
    repo: &str,
    lane: Lane,
    tree: &str,
    prefix: &str,
    windows: bool,
) -> Result<bool> {
    let taken = pictures(places, repo)
        .join(lane.word())
        .join(tree)
        .join(TAKEN);
    for (name, path) in entries_in(&taken)? {
        let (marked, took_windows) = match name.rsplit_once('@') {
            Some((marked, "windows")) => (marked, true),
            Some((marked, "no-windows")) => (marked, false),
            _ => fail!(
                "{} is no mark the shots program writes (<prefix>@windows or <prefix>@no-windows)",
                path.display()
            ),
        };
        if prefix.starts_with(marked) && (took_windows || !windows) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A new folder in the picture folder <pictures> for the pictures a run on a
/// host made, which `merge_pictures` then moves into place.
pub fn incoming(pictures: &Path) -> PathBuf {
    pictures.join(format!("{INCOMING}{}-{}", std::process::id(), now()))
}

/// Moves each picture file and taken mark in <incoming> that <pictures>
/// lacks to the same place in <pictures>, and deletes <incoming>. Element
/// lists go before pictures, so that the shots program never finds a picture
/// without its element list, and marks go last, so that builds never finds a
/// mark before the shots it says are kept. A file <pictures> already has
/// stays as it is: its key names the same inputs. A hidden file outside a
/// marks folder is one the shots program left half written: it stays out,
/// and the merge says so.
pub fn merge_pictures(incoming: &Path, pictures: &Path) -> Result<()> {
    let mut files = Vec::new();
    let mut folders = vec![incoming.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for (name, path) in entries_in(&folder)? {
            if path.is_dir() {
                folders.push(path);
            } else if name.starts_with('.') {
                note(format!(
                    "left out {}, a picture file the run on the host left half written",
                    path.display()
                ));
            } else {
                files.push(path);
            }
        }
    }
    let order = |file: &PathBuf| {
        if file.parent().is_some_and(|parent| parent.ends_with(TAKEN)) {
            2
        } else if file.extension().is_some_and(|ext| ext == "png") {
            1
        } else {
            0
        }
    };
    files.sort_by_key(|file| (order(file), file.clone()));
    for file in files {
        let relative = file
            .strip_prefix(incoming)
            .expect("the file was found in the folder");
        let into = pictures.join(relative);
        if into.exists() {
            continue;
        }
        if let Some(parent) = into.parent() {
            std::fs::create_dir_all(parent).context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::rename(&file, &into)
            .context(|| format!("moving {} to {}", file.display(), into.display()))?;
    }
    capped_store::remove_all(incoming)
}

/// The folder of the runs of <repo>'s <commit> on <lane>, one folder per
/// prefix.
fn prefixes_folder(places: &Places, repo: &str, commit: &str, lane: Lane) -> PathBuf {
    root(places)
        .join(repo)
        .join("runs")
        .join(commit)
        .join(lane.word())
}

/// The folder of the run <key>.
pub fn run_folder(places: &Places, key: &Key) -> PathBuf {
    prefixes_folder(places, &key.repo, &key.commit, key.lane)
        .join(&key.prefix)
        .join(&key.version)
}

/// A run's folder while the run fills it, beside the folder it replaces.
/// Dropped unfinished, it deletes what it holds.
pub struct Staging {
    key: Key,
    folder: PathBuf,
    inner: capped_store::Staging,
}

/// Takes the shared lock on `WRITING` that a run holds while it reads or
/// writes the folder `shots`, waiting while a prune holds it exclusively; the
/// lock lasts as long as the file it returns.
pub fn writing(places: &Places) -> Result<std::fs::File> {
    capped_store::writing(&root(places))
}

/// Starts storing a run of <key>: an empty staging folder that the run's
/// report goes into (`Staging::report`) before `Staging::finish` puts it in
/// place.
pub fn stage(places: &Places, key: &Key) -> Result<Staging> {
    check_prefix(&key.prefix)?;
    let folder = run_folder(places, key);
    Ok(Staging {
        key: key.clone(),
        inner: capped_store::stage(&root(places), folder.clone(), &key.version)?,
        folder,
    })
}

impl Staging {
    /// Where the run's report goes: a folder that does not exist yet.
    pub fn report(&self) -> PathBuf {
        self.inner.dir().join(REPORT)
    }

    /// Writes how the run ended, with exit status <exit_code>, and what the
    /// shots program printed (<findings>), and puts the folder in place of
    /// any earlier run of the key.
    pub fn finish(self, exit_code: i64, findings: &str) -> Result<Stored> {
        let recorded_at = now();
        let ending = Ending {
            recorded_at,
            exit_code,
        };
        let run_file = self.inner.dir().join(RUN_FILE);
        std::fs::write(
            &run_file,
            serde_json::to_string(&ending).expect("an ending serializes"),
        )
        .context(|| format!("writing {}", run_file.display()))?;
        let output = self.inner.dir().join(OUTPUT_FILE);
        std::fs::write(&output, findings).context(|| format!("writing {}", output.display()))?;
        self.inner.place()?;
        Ok(Stored {
            key: self.key,
            recorded_at,
            exit_code,
            findings: findings.to_string(),
            folder: self.folder,
        })
    }

    /// Deletes the report a copy left half made, so the run is kept without
    /// one.
    pub fn discard_report(&self) -> Result<()> {
        capped_store::remove_all(&self.report())
    }

    /// Deletes the staging folder and what it holds: the run is not kept.
    pub fn discard(self) -> Result<()> {
        self.inner.discard()
    }
}

/// The run in <folder>, stored under <key>; None when the folder holds no
/// finished run.
fn read(folder: &Path, key: &Key) -> Result<Option<Stored>> {
    let run_file = folder.join(RUN_FILE);
    let text = match std::fs::read_to_string(&run_file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("reading {}: {error}", run_file.display()),
    };
    let ending: Ending =
        serde_json::from_str(&text).context(|| format!("reading {}", run_file.display()))?;
    let output = folder.join(OUTPUT_FILE);
    let findings =
        std::fs::read_to_string(&output).context(|| format!("reading {}", output.display()))?;
    Ok(Some(Stored {
        key: key.clone(),
        recorded_at: ending.recorded_at,
        exit_code: ending.exit_code,
        findings,
        folder: folder.to_path_buf(),
    }))
}

/// Marks <stored> used now, so the cap keeps it longer.
fn mark_used(stored: &Stored) -> Result<()> {
    let run_file = stored.folder.join(RUN_FILE);
    std::fs::File::options()
        .write(true)
        .open(&run_file)
        .and_then(|file| file.set_modified(SystemTime::now()))
        .context(|| format!("marking {} used", run_file.display()))
}

/// The run stored under <key>, if any; reading it marks it used.
pub fn lookup(places: &Places, key: &Key) -> Result<Option<Stored>> {
    check_prefix(&key.prefix)?;
    let found = read(&run_folder(places, key), key)?;
    if let Some(stored) = &found {
        mark_used(stored)?;
    }
    Ok(found)
}

/// The run a branch's run of <prefix> on <lane> compares with: the newest
/// stored run of <repo>'s <commit> on that lane, by shots program <version>,
/// whose prefix starts <prefix> (a run of `mac.` stands in for one of
/// `mac.welcome.`) and whose report holds findings.json. Reading it marks it
/// used.
pub fn before_run(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    prefix: &str,
    version: &str,
) -> Result<Option<Stored>> {
    let folder = prefixes_folder(places, repo, commit, lane);
    let names = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("listing {}: {error}", folder.display()),
    };
    let mut found: Vec<Stored> = Vec::new();
    for entry in names {
        let entry = entry.context(|| format!("listing {}", folder.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !prefix.starts_with(&name) {
            continue;
        }
        let key = Key {
            repo: repo.to_string(),
            commit: commit.to_string(),
            lane,
            prefix: name,
            version: version.to_string(),
        };
        if let Some(stored) = read(&run_folder(places, &key), &key)?
            && stored.findings_json().is_file()
        {
            found.push(stored);
        }
    }
    let newest = found.into_iter().max_by_key(|stored| stored.recorded_at);
    if let Some(stored) = &newest {
        mark_used(stored)?;
    }
    Ok(newest)
}

/// The folders <dir> holds, with their names; nothing when <dir> is gone.
fn folders_in(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    Ok(entries_in(dir)?
        .into_iter()
        .filter(|(_, path)| path.is_dir() && !path.is_symlink())
        .collect())
}

/// The entries <dir> holds, with their names, sorted; nothing when <dir> is
/// gone.
fn entries_in(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", dir.display()))?;
        found.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
        ));
    }
    found.sort();
    Ok(found)
}

/// What the folder of <repo> in `shots` holds, as the cap sees it: its
/// runs, board pictures and app source trees' shots, and the leftovers of
/// runs that ended.
fn group(places: &Places, repo: &str) -> Result<Group> {
    let mut kept = Vec::new();
    for (_, commit) in folders_in(&root(places).join(repo).join("runs"))? {
        for (_, lane) in folders_in(&commit)? {
            for (_, prefix) in folders_in(&lane)? {
                for (name, folder) in folders_in(&prefix)? {
                    kept.push(if capped_store::is_staging(&name) {
                        Kept::Staging { folder }
                    } else {
                        Kept::Folder {
                            last_use: Use::At(written_at(&folder.join(RUN_FILE))?),
                            folder,
                            what: "a shots run",
                        }
                    });
                }
            }
        }
    }
    for (name, platform) in folders_in(&pictures(places, repo))? {
        if name.starts_with(INCOMING) {
            kept.push(Kept::Staging { folder: platform });
        } else if name != BOARD_PICTURES && !name.starts_with('.') {
            for (_, tree) in folders_in(&platform)? {
                kept.push(Kept::Folder {
                    folder: tree,
                    last_use: Use::NewestInside,
                    what: "an app source tree's shots",
                });
            }
        }
    }
    let boards = pictures(places, repo).join(BOARD_PICTURES);
    for (name, path) in entries_in(&boards)? {
        if name.starts_with('.') {
            kept.push(Kept::Loose { file: path });
        } else if let Some(hash) = name.strip_suffix(".png") {
            kept.push(Kept::Files {
                last_use: written_at(&path)?,
                // The picture goes before its element list, so the shots
                // program never finds a picture without one.
                files: vec![path.clone(), boards.join(format!("{hash}.json"))],
                what: "a board picture",
            });
        } else if let Some(hash) = name.strip_suffix(".json")
            && !boards.join(format!("{hash}.png")).exists()
        {
            kept.push(Kept::Loose { file: path });
        }
    }
    Ok(Group {
        folder: root(places).join(repo),
        kept,
    })
}

/// When <path> was last written, seconds since 1970.
fn written_at(path: &Path) -> Result<i64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .map(|meta| meta.mtime())
        .context(|| format!("reading the time of {}", path.display()))
}

/// Deletes the runs, board pictures and app source trees' shots that keep a
/// repo's folder in `shots` over `CAP_KIB` (prune.rs).
pub fn prune(tool: &Tool, run: Run) -> Result<()> {
    prune_runs(tool, run, CAP_KIB)
}

/// `capped_store::prune` of each repo's folder in `shots` with <cap_kib>.
pub fn prune_runs(tool: &Tool, run: Run, cap_kib: u64) -> Result<()> {
    let shots = root(&tool.places);
    if !shots.is_dir() {
        return Ok(());
    }
    let mut groups = Vec::new();
    for (repo, _) in folders_in(&shots)? {
        groups.push(group(&tool.places, &repo)?);
    }
    capped_store::prune(tool, run, &shots, cap_kib, groups)
}

#[cfg(test)]
#[path = "shot_store_tests.rs"]
mod tests;

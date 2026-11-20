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
//! picture depends on, and draws only the boards whose hash the folder lacks.
//!
//! builds keeps the folder `shots` under `CAP_KIB`: `setup` and `prune`
//! delete whole runs and board pictures, least recently used first, while it
//! is over (prune.rs calls `prune`). A run's last use is the time of its
//! run.json, which the run sets as it ends and each read sets again, and a
//! board picture's is the time of its picture, which the shots program sets
//! each time it takes the picture, so what agents keep using stays.
//!
//! A run that stores its output holds a shared lock on `shots/.writing`
//! (`WRITING`) from before it makes its folders until its run is in place,
//! and app-build holds it while the shots program writes to the picture
//! folder. The prune deletes staging folders, the picture files a run left
//! half written and the folders deleted runs leave empty only while it holds
//! that lock exclusively: then no run is writing any of them or about to make
//! a folder in one it would remove.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::app_timing::now;
use crate::evict::{Recreate, Removal};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::places::{FLOOR_IDLE_MINUTES, Places};
use crate::prune::{Run, delete_folder};
use crate::text::{human, note};
use crate::tool::Tool;

/// The folder on the build volume that holds every repo's runs.
pub const FOLDER: &str = "shots";

/// How much the folder `shots` may hold. A run of a lane's every shot keeps
/// about 10 MB of report, so the cap holds several hundred runs: every
/// recent commit of main and of the branches agents work on.
pub const CAP_KIB: u64 = 10 * 1024 * 1024;

/// The file of a run's folder that says how it ended.
const RUN_FILE: &str = "run.json";
/// The file of a run's folder that holds what the shots program printed.
const OUTPUT_FILE: &str = "output.txt";
/// The folder of a run's folder that holds its report.
const REPORT: &str = "report";
/// The folder of a picture folder that holds the board pictures.
const BOARD_PICTURES: &str = "boards";
/// What a staging folder's name starts with after the dot that hides it.
const STAGING: &str = ".staging-";
/// The lock file in the folder `shots` that each run storing its output
/// holds shared, and the prune exclusively while it deletes staging folders
/// and empty folders.
const WRITING: &str = ".writing";

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
    dir: PathBuf,
    into: PathBuf,
    /// The shared lock on `WRITING`, which the prune waits for before it
    /// deletes a staging folder or an empty folder.
    _writing: std::fs::File,
}

/// Takes the shared lock on `WRITING` that a run holds while it writes to
/// the folder `shots`, waiting while a prune holds it exclusively; the lock
/// lasts as long as the file it returns.
pub fn writing(places: &Places) -> Result<std::fs::File> {
    let shots = root(places);
    std::fs::create_dir_all(&shots).context(|| format!("creating {}", shots.display()))?;
    crate::evict::lock_shared_creating(&shots.join(WRITING))
}

/// Starts storing a run of <key>: an empty staging folder that the run's
/// report goes into (`Staging::report`) before `Staging::finish` puts it in
/// place.
pub fn stage(places: &Places, key: &Key) -> Result<Staging> {
    check_prefix(&key.prefix)?;
    let into = run_folder(places, key);
    let parent = into
        .parent()
        .expect("a run's folder is in its prefix's folder");
    let dir = parent.join(format!(
        "{STAGING}{}-{}-{}",
        key.version,
        std::process::id(),
        now()
    ));
    let writing = writing(places)?;
    remove_all(&dir)?;
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    Ok(Staging {
        key: key.clone(),
        dir,
        into,
        _writing: writing,
    })
}

impl Staging {
    /// Where the run's report goes: a folder that does not exist yet.
    pub fn report(&self) -> PathBuf {
        self.dir.join(REPORT)
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
        let run_file = self.dir.join(RUN_FILE);
        std::fs::write(
            &run_file,
            serde_json::to_string(&ending).expect("an ending serializes"),
        )
        .context(|| format!("writing {}", run_file.display()))?;
        let output = self.dir.join(OUTPUT_FILE);
        std::fs::write(&output, findings).context(|| format!("writing {}", output.display()))?;
        if self.into.exists() {
            std::fs::remove_dir_all(&self.into)
                .context(|| format!("removing {}", self.into.display()))?;
        }
        std::fs::rename(&self.dir, &self.into)
            .context(|| format!("moving {} to {}", self.dir.display(), self.into.display()))?;
        Ok(Stored {
            key: self.key.clone(),
            recorded_at,
            exit_code,
            findings: findings.to_string(),
            folder: self.into.clone(),
        })
    }
}

impl Staging {
    /// Deletes the report a copy left half made, so the run is kept without
    /// one.
    pub fn discard_report(&self) -> Result<()> {
        remove_all(&self.report())
    }

    /// Deletes the staging folder and what it holds: the run is not kept.
    pub fn discard(self) -> Result<()> {
        remove_all(&self.dir)
    }
}

/// Deletes <dir> and what it holds, if it is there.
pub fn remove_all(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            fail!("deleting {}: {error}", dir.display())
        }
        _ => Ok(()),
    }
}

/// A staging folder dropped unfinished, as on an early return or a panic,
/// goes. A deletion that fails is said, and the folder stays hidden beside
/// the run's folder until a prune that no run keeps out deletes it
/// (`prune_runs`). After `finish` or `discard` the folder is gone already.
impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(error) = remove_all(&self.dir) {
            note(format!(
                "{}, an unfinished shots run's staging folder",
                error.text()
            ));
        }
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

/// What the cap deletes: a run's folder, or a board picture with its
/// element list.
enum Unit {
    Run(PathBuf),
    Picture { png: PathBuf, json: PathBuf },
}

/// What the folder `shots` holds, as the cap sees it.
#[derive(Default)]
struct Held {
    /// Each run's folder.
    runs: Vec<PathBuf>,
    /// Each staging folder a run left or fills now.
    stagings: Vec<PathBuf>,
    /// Each board picture's picture and element list.
    pictures: Vec<(PathBuf, PathBuf)>,
    /// Each hidden file the shots program writes before it renames it into
    /// the picture folder, and each element list whose picture is gone.
    loose: Vec<PathBuf>,
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

/// What the folder `shots` of <places> holds.
fn held(places: &Places) -> Result<Held> {
    let mut held = Held::default();
    for (repo_name, repo) in folders_in(&root(places))? {
        for (_, commit) in folders_in(&repo.join("runs"))? {
            for (_, lane) in folders_in(&commit)? {
                for (_, prefix) in folders_in(&lane)? {
                    for (name, folder) in folders_in(&prefix)? {
                        if name.starts_with(STAGING) {
                            held.stagings.push(folder);
                        } else {
                            held.runs.push(folder);
                        }
                    }
                }
            }
        }
        let boards = pictures(places, &repo_name).join(BOARD_PICTURES);
        for (name, path) in entries_in(&boards)? {
            if name.starts_with('.') {
                held.loose.push(path);
            } else if let Some(hash) = name.strip_suffix(".png") {
                held.pictures
                    .push((path.clone(), boards.join(format!("{hash}.json"))));
            } else if let Some(hash) = name.strip_suffix(".json")
                && !boards.join(format!("{hash}.png")).exists()
            {
                held.loose.push(path);
            }
        }
    }
    Ok(held)
}

/// When <path> was last written, seconds since 1970.
fn written_at(path: &Path) -> Result<i64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .map(|meta| meta.mtime())
        .context(|| format!("reading the time of {}", path.display()))
}

/// The KiB the file <path> takes on disk; 0 when it is gone.
fn file_kib(path: &Path) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(meta) => Ok(meta.blocks() * 512 / 1024),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => fail!("reading the size of {}: {error}", path.display()),
    }
}

/// Deletes the runs and board pictures that keep the folder `shots` over
/// `CAP_KIB` (prune.rs).
pub fn prune(tool: &Tool, run: Run) -> Result<()> {
    prune_runs(tool, run, CAP_KIB)
}

/// Deletes each staging folder and loose picture file when no run stores
/// its output (the runs that wrote them ended), then, while the folder
/// `shots` holds more than <cap_kib>, whole runs and board pictures, least
/// recently used first, then the folders the deleted runs left empty when no
/// run stores its output. A run or picture used in the last
/// FLOOR_IDLE_MINUTES stays: an agent may be reading it.
pub fn prune_runs(tool: &Tool, run: Run, cap_kib: u64) -> Result<()> {
    let shots = root(&tool.places);
    if !shots.is_dir() {
        return Ok(());
    }
    // Held to the end, so that no run makes a folder while the prune removes
    // empty ones; None while a run stores its output.
    let alone = crate::evict::try_lock_creating(&shots.join(WRITING))?;
    if alone.is_none() {
        note(format!(
            "a shots run is storing its output in {}, so prune leaves its staging folders, half-written picture files and empty folders",
            shots.display()
        ));
    }
    let held = held(&tool.places)?;
    let mut paths = vec![shots.clone()];
    paths.extend(held.stagings.iter().cloned());
    paths.extend(held.runs.iter().cloned());
    let mut measures = tool.measure_all(&paths)?.into_iter();
    let mut total = measures.next().expect("the folder was measured").kib;
    for (folder, measured) in held.stagings.iter().zip(measures.by_ref()) {
        if alone.is_none() {
            continue;
        }
        let reason = format!(
            "{}, the staging folder of a shots run that ended",
            human(measured.kib)
        );
        if delete_folder(
            tool,
            folder,
            run,
            &reason,
            Recreate::No,
            Removal::Background,
            "",
        )? {
            total = total.saturating_sub(measured.kib);
        }
    }
    for file in held.loose.iter().filter(|_| alone.is_some()) {
        let kib = file_kib(file)?;
        let reason = format!(
            "{}, a picture file that a shots run which ended left half written",
            human(kib)
        );
        if delete_files(run, &[file], &reason)? {
            total = total.saturating_sub(kib);
        }
    }
    let mut units: Vec<(i64, u64, Unit)> = Vec::new();
    for (folder, measured) in held.runs.into_iter().zip(measures) {
        units.push((
            written_at(&folder.join(RUN_FILE))?,
            measured.kib,
            Unit::Run(folder),
        ));
    }
    for (png, json) in held.pictures {
        units.push((
            written_at(&png)?,
            file_kib(&png)? + file_kib(&json)?,
            Unit::Picture { png, json },
        ));
    }
    units.sort_by_key(|unit| unit.0);
    let recent = tool.now - (FLOOR_IDLE_MINUTES * 60) as i64;
    for (used, kib, unit) in units {
        if total <= cap_kib {
            break;
        }
        if used >= recent {
            continue;
        }
        let over = format!(
            "{} is {} > {}",
            shots.display(),
            human(total),
            human(cap_kib)
        );
        let deleted = match &unit {
            Unit::Run(folder) => delete_folder(
                tool,
                folder,
                run,
                &format!("{}, a shots run; {over}", human(kib)),
                Recreate::No,
                Removal::Background,
                "",
            )?,
            Unit::Picture { png, json } => delete_files(
                run,
                &[png, json],
                &format!("{}, a board picture; {over}", human(kib)),
            )?,
        };
        if deleted {
            total = total.saturating_sub(kib);
        }
    }
    if alone.is_some() && !run.dry() {
        remove_empty(tool)?;
    }
    Ok(())
}

/// Deletes <files> for <reason>, or says it would; returns whether they
/// went, or on a dry run would go. The picture goes before its element
/// list, so the shots program never finds a picture without one.
fn delete_files(run: Run, files: &[&PathBuf], reason: &str) -> Result<bool> {
    let shown = files[0].display();
    if run.dry() {
        note(format!("would delete {shown} ({reason})"));
        return Ok(true);
    }
    for file in files {
        match std::fs::remove_file(file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                fail!("deleting {}: {error}", file.display())
            }
            _ => {}
        }
    }
    note(format!("deleted {shown} ({reason})"));
    Ok(true)
}

/// Removes each folder under the folder `shots` that holds nothing: a
/// prefix's, a lane's, a commit's and a repo's. The caller holds `WRITING`
/// exclusively, so no run is about to make a folder in one of them.
fn remove_empty(tool: &Tool) -> Result<()> {
    let root = root(&tool.places);
    for (_, repo) in folders_in(&root)? {
        let runs = repo.join("runs");
        for (_, commit) in folders_in(&runs)? {
            for (_, lane) in folders_in(&commit)? {
                for (_, prefix) in folders_in(&lane)? {
                    remove_if_empty(&prefix)?;
                }
                remove_if_empty(&lane)?;
            }
            remove_if_empty(&commit)?;
        }
        remove_if_empty(&runs)?;
        remove_if_empty(&repo)?;
    }
    Ok(())
}

/// Removes <dir> when it holds nothing.
fn remove_if_empty(dir: &Path) -> Result<()> {
    match std::fs::remove_dir(dir) {
        Err(error)
            if !matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            fail!("removing {}: {error}", dir.display())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "shot_store_tests.rs"]
mod tests;

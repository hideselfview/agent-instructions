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
//! builds keeps the folder `shots` under `CAP_KIB`: `setup` and `prune`
//! delete whole runs, least recently used first, while it is over (prune.rs
//! calls `prune_runs`). A run's last use is the time of its run.json, which
//! the run sets as it ends and each read sets again, so a run that agents
//! keep reading stays.

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
use crate::text::human;
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
/// What a staging folder's name starts with after the dot that hides it.
const STAGING: &str = ".staging-";

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
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    Ok(Staging {
        key: key.clone(),
        dir,
        into,
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
fn remove_all(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            fail!("deleting {}: {error}", dir.display())
        }
        _ => Ok(()),
    }
}

/// A staging folder dropped unfinished, as on an early return or a panic,
/// goes; one whose deletion fails stays hidden beside the run's folder until
/// the cap deletes it (`prune_runs`).
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
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

/// A run's folder, or a staging folder a run left, as the cap sees it.
struct Kept {
    folder: PathBuf,
    staging: bool,
}

/// The folders <dir> holds, with their names; nothing when <dir> is gone.
fn folders_in(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() && !path.is_symlink() {
            found.push((entry.file_name().to_string_lossy().into_owned(), path));
        }
    }
    found.sort();
    Ok(found)
}

/// Every run's folder and staging folder in the folder `shots`.
fn kept(places: &Places) -> Result<Vec<Kept>> {
    let mut kept = Vec::new();
    for (_, repo) in folders_in(&root(places))? {
        for (_, commit) in folders_in(&repo.join("runs"))? {
            for (_, lane) in folders_in(&commit)? {
                for (_, prefix) in folders_in(&lane)? {
                    for (name, folder) in folders_in(&prefix)? {
                        kept.push(Kept {
                            staging: name.starts_with(STAGING),
                            folder,
                        });
                    }
                }
            }
        }
    }
    Ok(kept)
}

/// When <folder>'s run was last used, seconds since 1970: its run.json's
/// time, or 0 when it has none.
fn last_use(folder: &Path) -> i64 {
    std::fs::metadata(folder.join(RUN_FILE))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// Deletes the runs that keep the folder `shots` over `CAP_KIB` (prune.rs).
pub fn prune(tool: &Tool, run: Run) -> Result<()> {
    prune_runs(tool, run, CAP_KIB)
}

/// Deletes each staging folder nothing wrote to for FLOOR_IDLE_MINUTES (the
/// run that filled it died), then, while the folder `shots` holds more than
/// <cap_kib>, whole runs, least recently used first. A run used in the last
/// FLOOR_IDLE_MINUTES stays: an agent may be reading it.
pub fn prune_runs(tool: &Tool, run: Run, cap_kib: u64) -> Result<()> {
    let shots = root(&tool.places);
    if !shots.is_dir() {
        return Ok(());
    }
    let kept = kept(&tool.places)?;
    let mut paths = vec![shots.clone()];
    paths.extend(kept.iter().map(|kept| kept.folder.clone()));
    let mut measures = tool.measure_all(&paths)?.into_iter();
    let mut total = measures.next().expect("the folder was measured").kib;
    let mut runs = Vec::new();
    for (kept, measured) in kept.into_iter().zip(measures) {
        if !kept.staging {
            runs.push((last_use(&kept.folder), kept.folder, measured.kib));
        } else if !tool.written_within(&measured, FLOOR_IDLE_MINUTES) {
            let reason = format!(
                "{}, a shots run's staging folder that nothing wrote to in {FLOOR_IDLE_MINUTES}m",
                human(measured.kib)
            );
            if delete_folder(
                tool,
                &kept.folder,
                run,
                &reason,
                Recreate::No,
                Removal::Background,
                "",
            )? {
                total = total.saturating_sub(measured.kib);
            }
        }
    }
    runs.sort();
    let recent = tool.now - (FLOOR_IDLE_MINUTES * 60) as i64;
    for (used, folder, kib) in runs {
        if total <= cap_kib {
            break;
        }
        if used >= recent {
            continue;
        }
        let reason = format!(
            "{}, a shots run; {} is {} > {}",
            human(kib),
            shots.display(),
            human(total),
            human(cap_kib)
        );
        if delete_folder(
            tool,
            &folder,
            run,
            &reason,
            Recreate::No,
            Removal::Background,
            "",
        )? {
            total = total.saturating_sub(kib);
        }
    }
    if !run.dry() {
        remove_empty(tool)?;
    }
    Ok(())
}

/// Removes each folder under the folder `shots` that the deleted runs left
/// holding nothing and that nothing else changed in the last
/// FLOOR_IDLE_MINUTES (a run may be about to fill it): a prefix's, a lane's,
/// a commit's and a repo's. Each folder's time is read before the folders in
/// it go, since removing them changes it.
fn remove_empty(tool: &Tool) -> Result<()> {
    let root = root(&tool.places);
    for (_, repo) in folders_in(&root)? {
        let runs = repo.join("runs");
        let idle_repo = idle(tool, &repo)?;
        let idle_runs = idle(tool, &runs)?;
        for (_, commit) in folders_in(&runs)? {
            let idle_commit = idle(tool, &commit)?;
            for (_, lane) in folders_in(&commit)? {
                let idle_lane = idle(tool, &lane)?;
                for (_, prefix) in folders_in(&lane)? {
                    if idle(tool, &prefix)? {
                        remove_if_empty(&prefix)?;
                    }
                }
                if idle_lane {
                    remove_if_empty(&lane)?;
                }
            }
            if idle_commit {
                remove_if_empty(&commit)?;
            }
        }
        if idle_runs {
            remove_if_empty(&runs)?;
        }
        if idle_repo {
            remove_if_empty(&repo)?;
        }
    }
    Ok(())
}

/// Whether nothing changed in the folder <dir> in the last
/// FLOOR_IDLE_MINUTES; false when it is gone.
fn idle(tool: &Tool, dir: &Path) -> Result<bool> {
    match std::fs::metadata(dir).and_then(|meta| meta.modified()) {
        Ok(time) => {
            let changed = time
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|since| since.as_secs() as i64)
                .unwrap_or(0);
            Ok(changed < tool.now - (FLOOR_IDLE_MINUTES * 60) as i64)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => fail!("reading {}: {error}", dir.display()),
    }
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

//! The cache of the output of each `--shots` and `--all-shots` run: on
//! the build volume, never in a project's lists store. A run is build output,
//! cached so the next agent that wants the run of a commit, or the before side
//! of a branch's run, reads it instead of building.
//!
//! Each run has one folder, found by what made it:
//!
//!   <volume>/shots/<repo>/runs/<commit>/<lane>/<environment>/<prefix>/<capture>/<comparison>/
//!     run-v2.json  when the run ended, its kind and the shots command's exit status
//!     output.txt   what the shots program printed, findings and verdict
//!     report/      the program's report as it wrote it: index.html, the
//!                  pictures that page shows and findings.json
//!
//! <commit> is the full commit id the run built, <environment> the host
//! software that drew the shots (shots_environment.rs), <prefix> the shot-name
//! prefix it was given (`mac.` for a lane's every shot), <capture> the capture
//! version of the shots program that took the shots and drew the boards, and
//! <comparison> the comparison version of the program that compared them
//! (shots_program.rs `Versions`). A run compares only with a run made in the
//! same environment: another environment draws the same app's shots with other
//! element labels and other pixels. Under one <capture> folder, each
//! <comparison> folder holds the same pictures with its own findings:
//! `compare_again` (shots_compare_again.rs) makes a run of the current
//! comparison version from a run of another, so a change to the comparison
//! takes no shot and draws no board again. A later run of the same key
//! replaces the folder whole: a run fills a staging folder at the top of
//! `shots` and moves it into place only when it is complete, so a reader
//! finds either the old run or the new one, and a run that stops or caches
//! nothing leaves no folder under `runs` (capped_store.rs).
//!
//! Beside the runs, each repo has a picture folder,
//! `<volume>/shots/<repo>/pictures/`, which app-build passes to the shots
//! program (`run --pictures`): the program caches each board picture it draws
//! there as `boards/<board hash>.png` and `.json`, the hash of everything the
//! picture depends on, and draws only the boards whose hash the folder lacks;
//! and it caches each shot the app takes as
//! `<platform>/<app source tree>/<shot>.png` and `.json` (app_tree.rs), and
//! takes no shot again when the folder caches every one the run selects. The
//! folder's key for a shot is the app source tree combined with the
//! environment (`app_tree::in_environment`), so builds reuses a cached shot
//! only in the environment that took it.
//!
//! builds holds each repo's folder in `shots` under `CAP_KIB`: `setup` and
//! `prune` delete the repo's whole runs, board pictures and app source
//! trees' shots, least recently used first, while it is over (prune.rs calls
//! `prune`). A run's last use is the time of its run-v2.json, which the run
//! sets as it ends and each read sets again; a board picture's is the time of
//! its picture, and an app source tree's shots' the newest time in their
//! folder, which the shots program sets each time it takes them; so what
//! agents keep using stays.
//!
//! The prune never deletes a run that a branch waiting to land names, as the
//! run that covers one of the branch's app builds or as the run that build
//! compared with (shots_in_use.rs), however long ago the run was used. When
//! those runs alone pass `CAP_KIB`, the prune keeps them all and `builds
//! health` names the branches (`over_cap_in_use`).
//!
//! The folder `shots` is a capped store (capped_store.rs): its lock on
//! `shots/.writing` is held by each run that caches its output, and by
//! app-build while a shots step reads a before run or the picture folder or
//! writes to it, so the prune deletes nothing a run reads or writes.
//! A lane holds its app tree separately from before it checks its taken
//! marks through its last comparison of that tree (`AppPictures`), so
//! pruning other pictures between its steps leaves the ones it needs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use shots::{CAPTURE_MARKS_DIR, CaptureMarks};

use crate::app_timing::now;
use crate::capped_store::{self, Group, Kept, Use, entries_in, folders_in};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::prune::Run;
use crate::shots_in_use::{self, Named};
use crate::shots_program::Versions;
use crate::text::note;
use crate::tool::Tool;

/// The folder on the build volume that holds every repo's runs.
pub const FOLDER: &str = "shots";

/// How much each repo's folder in `shots` may hold, its runs and its picture
/// folder together. A run of a lane's every shot takes about 10 MB of
/// report, so the cap holds main's runs and many branch runs.
pub const CAP_KIB: u64 = 2 * 1024 * 1024;

/// The file of a run's folder that says how it ended.
pub(crate) const RUN_FILE: &str = "run-v2.json";
/// The file of a run's folder that holds what the shots program printed.
const OUTPUT_FILE: &str = "output.txt";
/// The folder of a run's folder that holds its report.
const REPORT: &str = "report";
/// The folder of a picture folder that holds the board pictures.
pub const BOARD_PICTURES: &str = "boards";
/// What the name of a folder starts with that holds, in a picture folder,
/// the pictures a run on a host made while builds moves them into place.
const INCOMING: &str = ".incoming-";

/// What a cached run is found by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub repo: String,
    /// The full commit id.
    pub commit: String,
    pub lane: Lane,
    /// The host software that drew the run's shots (shots_environment.rs).
    pub environment: String,
    pub prefix: String,
    /// The versions of the shots program that made the run's shots and board
    /// pictures (capture) and its findings (comparison).
    pub versions: Versions,
}

impl Key {
    /// The key of a run of <prefix> on <lane> for <repo>'s <commit> made in
    /// <environment> by the shots program <versions>.
    pub fn new(
        repo: &str,
        commit: &str,
        lane: Lane,
        environment: &str,
        prefix: &str,
        versions: &Versions,
    ) -> Key {
        Key {
            repo: repo.to_string(),
            commit: commit.to_string(),
            lane,
            environment: environment.to_string(),
            prefix: prefix.to_string(),
            versions: versions.clone(),
        }
    }
}

/// The environments a read takes runs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Made<'a> {
    /// Only runs made in this environment: a comparison (`--before`) needs
    /// the run of its own environment.
    In(&'a str),
    /// Runs made in any environment, for a read of a commit's own run, which
    /// prints the environment it was made in.
    Anywhere,
}

/// What run-v2.json holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Ending {
    /// When the run ended, seconds since 1970.
    recorded_at: i64,
    /// The shots command's exit status: 0 when no pair failed the run.
    exit_code: u8,
    kind: RunKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RunKind {
    Comparison,
    Recheck { source_exit_code: u8 },
}

/// A cached run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached {
    pub key: Key,
    ending: Ending,
    pub findings: String,
    /// The run's folder.
    pub folder: PathBuf,
}

impl Cached {
    pub fn recorded_at(&self) -> i64 {
        self.ending.recorded_at
    }

    pub fn exit_code(&self) -> u8 {
        self.ending.exit_code
    }

    /// The original comparison's verdict. A recheck compares without the
    /// base, so its own status cannot replace that verdict.
    pub fn app_exit_code(&self) -> u8 {
        match self.ending.kind {
            RunKind::Comparison => self.ending.exit_code,
            RunKind::Recheck { source_exit_code } => source_exit_code,
        }
    }

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

/// Fails unless <key>'s prefix and environment can each be one folder's name.
fn check_key(key: &Key) -> Result<()> {
    let (prefix, environment) = (&key.prefix, &key.environment);
    if prefix.is_empty() || prefix.contains('/') || prefix.starts_with('.') {
        fail!("{prefix:?} is no shot-name prefix: a prefix starts with a platform and holds no /");
    }
    if environment.is_empty() || environment.contains('/') || environment.starts_with('.') {
        fail!("{environment:?} is no environment: it names one folder and holds no /");
    }
    Ok(())
}

/// The picture folder of <repo>, which the shots program reads and adds
/// board pictures to.
pub fn pictures(places: &Places, repo: &str) -> PathBuf {
    root(places).join(repo).join("pictures")
}

/// The app pictures of one tree and the directory the lane locks while it
/// captures or compares them.
pub struct AppPictures {
    root: PathBuf,
    marks: CaptureMarks,
}

impl AppPictures {
    pub fn of(places: &Places, repo: &str, lane: Lane, tree: &str) -> Self {
        let dir = app_picture_tree(places, repo, lane, tree);
        Self {
            root: root(places),
            marks: CaptureMarks::of(&dir),
        }
    }

    /// Keep this tree in the cache while the lane runs <work>.
    pub fn while_held<T>(
        &self,
        query: Option<&CaptureQuery>,
        work: impl FnOnce(bool) -> T,
    ) -> PictureLeaseResult<T> {
        let _reader = {
            // Take the shared lock while prune cannot replace the directory inode.
            let _root = capped_store::writing(&self.root)?;
            let marks = self.marks.directory();
            std::fs::create_dir_all(marks).context(|| format!("creating {}", marks.display()))?;
            let reader =
                std::fs::File::open(marks).context(|| format!("opening {}", marks.display()))?;
            crate::flock::lock(&reader, libc::LOCK_SH, marks)?;
            reader
        };
        let cached = match query {
            Some(query) => self.marks.cover(&query.prefixes, query.windows)?,
            None => false,
        };
        Ok(work(cached))
    }
}

#[derive(Debug)]
pub enum PictureLeaseError {
    Access(Fail),
    Mark(shots::CaptureMarkError),
}

impl From<Fail> for PictureLeaseError {
    fn from(error: Fail) -> Self {
        Self::Access(error)
    }
}

impl From<shots::CaptureMarkError> for PictureLeaseError {
    fn from(error: shots::CaptureMarkError) -> Self {
        Self::Mark(error)
    }
}

impl From<PictureLeaseError> for Fail {
    fn from(error: PictureLeaseError) -> Self {
        match error {
            PictureLeaseError::Access(error) => error,
            PictureLeaseError::Mark(error) => Fail::Message(error.to_string()),
        }
    }
}

pub type PictureLeaseResult<T> = std::result::Result<T, PictureLeaseError>;

pub struct CaptureQuery {
    pub prefixes: Vec<shots::Prefix>,
    pub windows: bool,
}

pub(super) fn app_picture_tree(places: &Places, repo: &str, lane: Lane, tree: &str) -> PathBuf {
    shots::app_picture_tree(&pictures(places, repo), lane.shots_platform(), tree)
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
/// mark before the shots it says are cached. A file <pictures> already has
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
        if file
            .parent()
            .is_some_and(|parent| parent.ends_with(CAPTURE_MARKS_DIR))
        {
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
/// environment.
fn lane_folder(places: &Places, repo: &str, commit: &str, lane: Lane) -> PathBuf {
    root(places)
        .join(repo)
        .join("runs")
        .join(commit)
        .join(lane.word())
}

/// The folder of the runs of <repo>'s <commit> on <lane> made in
/// <environment>, one folder per prefix.
fn prefixes_folder(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    environment: &str,
) -> PathBuf {
    lane_folder(places, repo, commit, lane).join(environment)
}

/// The environments that made runs of <repo>'s <commit> on <lane>.
pub fn environments(places: &Places, repo: &str, commit: &str, lane: Lane) -> Result<Vec<String>> {
    Ok(folders_in(&lane_folder(places, repo, commit, lane))?
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| !name.starts_with('.'))
        .collect())
}

/// The folder of the runs of <key>'s capture version: one folder for each
/// comparison version that compared its pictures.
fn captures_folder(places: &Places, key: &Key) -> PathBuf {
    prefixes_folder(places, &key.repo, &key.commit, key.lane, &key.environment)
        .join(&key.prefix)
        .join(&key.versions.capture)
}

/// The folder of the run <key>.
pub fn run_folder(places: &Places, key: &Key) -> PathBuf {
    captures_folder(places, key).join(&key.versions.comparison)
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

/// Starts caching a run of <key>: an empty staging folder that the run's
/// report goes into (`Staging::report`) before `Staging::finish` puts it in
/// place.
pub fn stage(places: &Places, key: &Key) -> Result<Staging> {
    check_key(key)?;
    let folder = run_folder(places, key);
    Ok(Staging {
        key: key.clone(),
        inner: capped_store::stage(&root(places), folder.clone(), &key.versions.comparison)?,
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
    pub fn finish(self, exit_code: i64, findings: &str) -> Result<Cached> {
        self.finish_at(exit_code, findings, now())
    }

    pub(crate) fn finish_at(
        self,
        exit_code: i64,
        findings: &str,
        recorded_at: i64,
    ) -> Result<Cached> {
        let exit_code = u8::try_from(exit_code)
            .context(|| "recording the shots comparison's exit status".to_string())?;
        self.finish_run(
            Ending {
                recorded_at,
                exit_code,
                kind: RunKind::Comparison,
            },
            findings,
        )
    }

    pub fn finish_recheck(self, source: &Cached, passed: bool, findings: &str) -> Result<Cached> {
        self.finish_run(
            Ending {
                recorded_at: now(),
                exit_code: u8::from(!passed),
                kind: RunKind::Recheck {
                    source_exit_code: source.app_exit_code(),
                },
            },
            findings,
        )
    }

    fn finish_run(self, ending: Ending, findings: &str) -> Result<Cached> {
        let run_file = self.inner.dir().join(RUN_FILE);
        std::fs::write(
            &run_file,
            serde_json::to_string(&ending).expect("an ending serializes"),
        )
        .context(|| format!("writing {}", run_file.display()))?;
        let output = self.inner.dir().join(OUTPUT_FILE);
        std::fs::write(&output, findings).context(|| format!("writing {}", output.display()))?;
        self.inner.place()?;
        Ok(Cached {
            key: self.key,
            ending,
            findings: findings.to_string(),
            folder: self.folder,
        })
    }

    /// Deletes the report a copy left half made, so the run is cached without
    /// one.
    pub fn discard_report(&self) -> Result<()> {
        capped_store::remove_all(&self.report())
    }

    /// Deletes the staging folder and what it holds: the run is not cached.
    pub fn discard(self) -> Result<()> {
        self.inner.discard()
    }
}

/// The run in <folder>, cached under <key>; None when the folder holds no
/// finished run.
fn read(folder: &Path, key: &Key) -> Result<Option<Cached>> {
    let run_file = folder.join(RUN_FILE);
    let text = match std::fs::read_to_string(&run_file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("reading {}: {error}", run_file.display()),
    };
    let ending =
        serde_json::from_str(&text).context(|| format!("reading {}", run_file.display()))?;
    let output = folder.join(OUTPUT_FILE);
    let findings =
        std::fs::read_to_string(&output).context(|| format!("reading {}", output.display()))?;
    Ok(Some(Cached {
        key: key.clone(),
        ending,
        findings,
        folder: folder.to_path_buf(),
    }))
}

/// Marks <cached> used now, so the cap deletes it later.
fn mark_used(cached: &Cached) -> Result<()> {
    capped_store::mark_used(&cached.folder.join(RUN_FILE))
}

/// The last use of a published run, or none for output without a verdict.
fn last_used_at(folder: &Path) -> Result<Option<i64>> {
    capped_store::written_at(&folder.join(RUN_FILE))
}

/// Reads a covering run in any environment with the current shots program
/// versions. The cache lease protects both the read and its last-use renewal.
pub fn read_covering<T>(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    prefix: &str,
    versions: &Versions,
    read: impl FnOnce(&Cached) -> Result<Option<T>>,
) -> Result<Option<T>> {
    let _reading = writing(places)?;
    let Some(cached) = covering_run(places, repo, commit, lane, Made::Anywhere, prefix, versions)?
    else {
        return Ok(None);
    };
    let read = read(&cached)?;
    if read.is_some() {
        mark_used(&cached)?;
    }
    Ok(read)
}

/// The newest run of <repo>'s <commit>, <lane> and exactly <prefix> that any
/// shots program made, in any environment: the run a reader of its findings
/// takes when this builds' program made none. Reading it marks it used.
pub fn lookup_older_program(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    prefix: &str,
) -> Result<Option<Cached>> {
    let mut newest: Option<Cached> = None;
    for environment in environments(places, repo, commit, lane)? {
        let prefix_folder = prefixes_folder(places, repo, commit, lane, &environment).join(prefix);
        for (capture, captures) in folders_in(&prefix_folder)? {
            for (comparison, folder) in folders_in(&captures)? {
                let key = Key::new(
                    repo,
                    commit,
                    lane,
                    &environment,
                    prefix,
                    &Versions {
                        capture: capture.clone(),
                        comparison,
                    },
                );
                if let Some(cached) = read(&folder, &key)?
                    && newest
                        .as_ref()
                        .is_none_or(|known| cached.recorded_at() > known.recorded_at())
                {
                    newest = Some(cached);
                }
            }
        }
    }
    if let Some(cached) = &newest {
        mark_used(cached)?;
    }
    Ok(newest)
}

/// The run cached under <key>, if any; reading it marks it used.
pub fn lookup(places: &Places, key: &Key) -> Result<Option<Cached>> {
    check_key(key)?;
    let found = read(&run_folder(places, key), key)?;
    if let Some(cached) = &found {
        mark_used(cached)?;
    }
    Ok(found)
}

/// The run covering <prefix> that matches <made>. Reading it marks it used.
pub fn covering_run_used(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    made: Made<'_>,
    prefix: &str,
    versions: &Versions,
) -> Result<Option<Cached>> {
    let found = covering_run(places, repo, commit, lane, made, prefix, versions)?;
    if let Some(cached) = &found {
        mark_used(cached)?;
    }
    Ok(found)
}

/// The keys of the runs of <repo>'s <commit> on <lane>, made in <made>, by
/// shots program <versions>, whose prefix starts <prefix>: the runs that can
/// cover it.
pub fn covering_keys(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    made: Made,
    prefix: &str,
    versions: &Versions,
) -> Result<Vec<Key>> {
    let environments = match made {
        Made::In(environment) => vec![environment.to_string()],
        Made::Anywhere => environments(places, repo, commit, lane)?,
    };
    let mut found = Vec::new();
    for environment in environments {
        let folder = prefixes_folder(places, repo, commit, lane, &environment);
        for (name, _) in folders_in(&folder)? {
            if name.starts_with('.') || !prefix.starts_with(&name) {
                continue;
            }
            found.push(Key::new(repo, commit, lane, &environment, &name, versions));
        }
    }
    Ok(found)
}

/// The folders of the runs, made in any environment, that can cover <prefix>
/// (`covering_keys`), of <versions>' capture version and every comparison
/// version: the runs another comparison version made are what
/// `compare_again` compares again.
pub fn covering_folders(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    prefix: &str,
    versions: &Versions,
) -> Result<Vec<PathBuf>> {
    let made = Made::Anywhere;
    let mut folders = Vec::new();
    for key in covering_keys(places, repo, commit, lane, made, prefix, versions)? {
        for (_, folder) in folders_in(&captures_folder(places, &key))? {
            if last_used_at(&folder)?.is_some() {
                folders.push(folder);
            }
        }
    }
    Ok(folders)
}

/// The runs of <key>'s commit, lane, environment, prefix and capture version
/// that another comparison version made and whose report holds
/// findings.json, newest first: the runs that `compare_again` compares again
/// for <key>. A run made in another environment is no source: its pictures
/// are another environment's.
pub fn other_comparisons(places: &Places, key: &Key) -> Result<Vec<Cached>> {
    let mut found = Vec::new();
    for (name, folder) in folders_in(&captures_folder(places, key))? {
        if name == key.versions.comparison {
            continue;
        }
        let other = Key {
            versions: Versions {
                comparison: name,
                ..key.versions.clone()
            },
            ..key.clone()
        };
        if let Some(cached) = read(&folder, &other)?
            && cached.findings_json().is_file()
        {
            found.push(cached);
        }
    }
    found.sort_by_key(|cached| std::cmp::Reverse(cached.recorded_at()));
    Ok(found)
}

/// The newest cached run of <repo>'s <commit> on <lane>, made in <made>, by
/// shots program <versions>, whose prefix starts <prefix> (a run of `mac.`
/// stands in for one of `mac.welcome.`) and whose report holds findings.json.
/// Finding it writes nothing: a caller that uses the run marks it used
/// (`mark_used`).
pub fn covering_run(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    made: Made,
    prefix: &str,
    versions: &Versions,
) -> Result<Option<Cached>> {
    let mut found: Vec<Cached> = Vec::new();
    for key in covering_keys(places, repo, commit, lane, made, prefix, versions)? {
        if let Some(cached) = read(&run_folder(places, &key), &key)?
            && cached.findings_json().is_file()
        {
            found.push(cached);
        }
    }
    Ok(found.into_iter().max_by_key(Cached::recorded_at))
}

/// What the folder of <repo> in `shots` holds, as the cap sees it: its
/// runs, board pictures and app source trees' shots, and the leftovers of
/// runs that ended.
fn group(places: &Places, repo: &str, named: &Named) -> Result<Group> {
    let mut kept = Vec::new();
    for (_, commit) in folders_in(&root(places).join(repo).join("runs"))? {
        for (_, lane) in folders_in(&commit)? {
            for (_, environment) in folders_in(&lane)? {
                for (_, prefix) in folders_in(&environment)? {
                    for (_, capture) in folders_in(&prefix)? {
                        for (_, folder) in folders_in(&capture)? {
                            let Some(last_use) = last_used_at(&folder)? else {
                                kept.push(Kept::Folder {
                                    folder: capped_store::Folder::unpinned(folder),
                                    last_use: Use::NewestInside,
                                    what: "shots output without a current verdict",
                                });
                                continue;
                            };
                            kept.push(match named.get(&folder) {
                                Some(branches) => Kept::InUse {
                                    by: branches.iter().cloned().collect(),
                                    folder,
                                },
                                None => Kept::Folder {
                                    folder: capped_store::Folder::unpinned(folder),
                                    last_use: Use::At(last_use),
                                    what: "a shots run",
                                },
                            });
                        }
                    }
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
                    folder: capped_store::Folder {
                        reader_lock: Some(CaptureMarks::of(&tree).directory().to_path_buf()),
                        path: tree,
                    },
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
            let Some(last_use) = capped_store::written_at(&path)? else {
                note(format!(
                    "{} disappeared during prune planning",
                    path.display()
                ));
                continue;
            };
            kept.push(Kept::Files {
                last_use,
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

/// Deletes the runs, board pictures and app source trees' shots that hold a
/// repo's folder in `shots` over `CAP_KIB` (prune.rs).
pub fn prune(tool: &Tool, run: Run) -> Result<()> {
    prune_runs(tool, run, CAP_KIB)
}

/// `capped_store::prune` of each repo's folder in `shots` with <cap_kib>.
pub fn prune_runs(tool: &Tool, run: Run, cap_kib: u64) -> Result<()> {
    prune_naming(tool, run, cap_kib, &|repo| {
        shots_in_use::named(&tool.places, repo)
    })
}

/// `prune_runs`, with <named> giving the runs of a repo that branches name.
fn prune_naming(
    tool: &Tool,
    run: Run,
    cap_kib: u64,
    named: &dyn Fn(&str) -> Result<Named>,
) -> Result<()> {
    let shots = root(&tool.places);
    if !shots.is_dir() {
        return Ok(());
    }
    capped_store::prune(tool, run, &shots, cap_kib, || {
        capped_store::tops_in(&shots)?
            .into_iter()
            .map(|(repo, _)| group(&tool.places, &repo, &named(&repo)?))
            .collect::<Result<Vec<_>>>()
            .map(Some)
    })
}

/// A repo whose runs that branches waiting to land name take more than
/// `CAP_KIB`, so the prune keeps all of them and the folder stays over its
/// cap.
#[derive(Debug, PartialEq, Eq)]
pub struct OverCap {
    pub repo: String,
    pub kib: u64,
    /// The branches that name a run, sorted.
    pub branches: Vec<String>,
}

/// The repos whose runs in use alone pass `CAP_KIB`, for `builds health`.
pub fn over_cap_in_use(tool: &Tool) -> Result<Vec<OverCap>> {
    let shots = root(&tool.places);
    let mut over = Vec::new();
    for (repo, folder) in capped_store::tops_in(&shots)? {
        // The runs in use are part of the folder: a folder under its cap
        // holds no more than the cap of them.
        if tool.measure(&folder)?.kib <= CAP_KIB {
            continue;
        }
        let named = shots_in_use::named(&tool.places, &repo)?;
        let folders: Vec<PathBuf> = named.keys().cloned().collect();
        let kib: u64 = tool.measure_all(&folders)?.iter().map(|m| m.kib).sum();
        if kib > CAP_KIB {
            let branches: BTreeSet<&String> = named.values().flatten().collect();
            over.push(OverCap {
                repo,
                kib,
                branches: branches.into_iter().cloned().collect(),
            });
        }
    }
    Ok(over)
}

#[cfg(test)]
#[path = "shots_cache_tests.rs"]
mod tests;

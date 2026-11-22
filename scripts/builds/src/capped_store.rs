//! A folder on the build volume that holds build output builds can make
//! again, bounded by a size cap: the shots runs (shots_cache.rs) and the
//! review texts (review/texts.rs) are two of them. The folder holds units,
//! each one folder that a writer fills whole and that the cap deletes whole.
//!
//! A writer fills a staging folder at the top of the root and moves it into
//! place only when it is complete, making the folders above the place then
//! and replacing an earlier unit of the same place, so a reader finds either
//! the old unit or the new one (`stage`, `Staging::place`). A writer that
//! stops, or keeps nothing, leaves no folder under the unit's place.
//!
//! A writer or reader holds a shared lock on `<root>/.writing` (`WRITING`,
//! `writing`) from before it reads or makes anything in the root until it is
//! done. The prune deletes anything in the root only while it holds that lock
//! exclusively: then nobody reads what it deletes, fills a staging folder or
//! is about to make a folder in one the prune removes, so each staging folder
//! there is one a writer that ended left.
//!
//! `setup` and `prune` delete units, least recently used first, while a group
//! of the root is over its cap (`prune`). A unit used in the last
//! FLOOR_IDLE_MINUTES stays, and so does a unit marked in use (`Kept::InUse`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::evict::HeldPaths;
use crate::fail;
use crate::fail::{Context, Result, with_cleanup};
use crate::places::FLOOR_IDLE_MINUTES;
use crate::prune::Run;
use crate::text::{human, note};
use crate::tool::Tool;
use crate::whole_file::remove_if_present;

/// The lock file in a root that each writer holds shared and the prune
/// exclusively while it deletes staging folders and empty folders.
const WRITING: &str = ".writing";
/// Serializes prune plans without excluding readers and writers.
const PRUNING: &str = ".pruning";
/// An access removes this file before touching the cache. A prune can use
/// its plan only while the file it created still exists.
const PLAN: &str = ".prune-plan";
/// What a staging folder's name starts with after the dot that hides it.
const STAGING: &str = ".staging-";

/// Whether <name>, a folder's name, is a staging folder's.
fn is_staging(name: &str) -> bool {
    name.starts_with(STAGING)
}

/// A unit's folder while a writer fills it, at the top of the root. Dropped
/// unfinished, it deletes what it holds.
pub struct Staging {
    dir: PathBuf,
    into: PathBuf,
    /// The shared lock on `WRITING`, which the prune waits for before it
    /// deletes a staging folder or an empty folder.
    _writing: std::fs::File,
}

/// Takes the shared lock on `WRITING` in <root>, creating the folder, that a
/// writer or reader holds while it works there, waiting while a prune holds it
/// exclusively; the lock lasts as long as the file it returns.
pub fn writing(root: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(root).context(|| format!("creating {}", root.display()))?;
    let shared = crate::evict::lock_shared_creating(&root.join(WRITING))?;
    remove_if_present(&root.join(PLAN))?;
    Ok(shared)
}

/// Starts storing a unit at <into>, under <root>: an empty staging folder at
/// the top of <root>, named for <tag>, that the writer fills
/// (`Staging::dir`) before `Staging::place` puts it in place.
pub fn stage(root: &Path, into: PathBuf, tag: &str) -> Result<Staging> {
    let writing = writing(root)?;
    let dir = root.join(format!("{STAGING}{tag}-{}", crate::text::staged_suffix()));
    remove_all(&dir)?;
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    Ok(Staging {
        dir,
        into,
        _writing: writing,
    })
}

impl Staging {
    /// The staging folder, which the writer fills.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Puts the folder in place of any earlier unit there, making the
    /// folders above the place.
    pub fn place(self) -> Result<()> {
        let parent = self.into.parent().expect("a unit's folder is in a folder");
        std::fs::create_dir_all(parent).context(|| format!("creating {}", parent.display()))?;
        if self.into.exists() {
            std::fs::remove_dir_all(&self.into)
                .context(|| format!("removing {}", self.into.display()))?;
        }
        std::fs::rename(&self.dir, &self.into)
            .context(|| format!("moving {} to {}", self.dir.display(), self.into.display()))
    }

    /// Deletes the staging folder and what it holds: the unit is not kept.
    pub fn discard(self) -> Result<()> {
        remove_all(&self.dir)
    }
}

/// The entries <dir> holds, with their names, sorted; nothing when <dir> is
/// gone.
pub fn entries_in(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
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

/// The folders <dir> holds, with their names, sorted; nothing when <dir> is
/// gone.
pub fn folders_in(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    Ok(entries_in(dir)?
        .into_iter()
        .filter(|(_, path)| path.is_dir() && !path.is_symlink())
        .collect())
}

/// The folders at the top of <root> that hold its groups' units, with their
/// names: every folder there but the hidden ones, such as a writer's staging
/// folder.
pub fn tops_in(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    Ok(folders_in(root)?
        .into_iter()
        .filter(|(name, _)| !name.starts_with('.'))
        .collect())
}

/// Sets the time of <file> to now: marks the unit it is in used, so the cap
/// keeps it longer.
pub fn mark_used(file: &Path) -> Result<()> {
    std::fs::File::options()
        .write(true)
        .open(file)
        .and_then(|file| file.set_modified(std::time::SystemTime::now()))
        .context(|| format!("marking {} used", file.display()))
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
/// goes. A deletion that fails is said, and the folder stays hidden at the
/// top of the root until a prune that no writer keeps out deletes it. After
/// `place` or `discard` the folder is gone already.
impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(error) = remove_all(&self.dir) {
            note(format!(
                "{}, an unfinished unit's staging folder",
                error.text()
            ));
        }
    }
}

/// When a unit was last used.
pub enum Use {
    /// At a time, seconds since 1970.
    At(i64),
    /// When the newest file inside the folder was written.
    NewestInside,
}

/// The folder a capped unit occupies and the lock its readers hold, if any.
pub struct Folder {
    pub path: PathBuf,
    /// Readers acquire this lock while holding WRITING.
    pub reader_lock: Option<PathBuf>,
}

/// What a group of a root holds, as the cap sees it.
pub enum Kept {
    /// A unit that is one folder; <what> names it in the reason its deletion
    /// gives.
    Folder {
        folder: Folder,
        last_use: Use,
        what: &'static str,
    },
    /// A unit that is one folder and that a reader the cap cannot see still
    /// needs, the branches of <by>: the cap never deletes it, but counts its
    /// size.
    InUse { folder: PathBuf, by: Vec<String> },
    /// A unit that is several files in one directory, deleted in order.
    Files {
        files: Vec<PathBuf>,
        last_use: i64,
        what: &'static str,
    },
    /// A folder a writer of the group left half filled, or fills now.
    Staging { folder: PathBuf },
    /// A file a writer left half written.
    Loose { file: PathBuf },
}

/// A folder under a root that has its own cap, and what it holds.
pub struct Group {
    pub folder: PathBuf,
    pub kept: Vec<Kept>,
}

/// What the cap deletes of a group.
enum Unit {
    Folder(Folder),
    Files(Vec<PathBuf>),
}

struct Deletion {
    unit: Unit,
    reason: String,
}

struct MeasuredUnit {
    used: i64,
    kib: u64,
    what: &'static str,
    unit: Unit,
}

struct MeasuredGroup {
    folder: PathBuf,
    total: u64,
    abandoned: Vec<(Deletion, u64)>,
    units: Vec<MeasuredUnit>,
}

/// Deletes nothing while a writer or reader holds <root>; else each staging
/// folder at the top of <root> and, in each of <groups>, each staging folder
/// and loose file (the writers that made them ended), then, while the group's folder holds more than <cap_kib>, whole
/// units, least recently used first, and last the folders under <root> that
/// the deletions left empty. A unit used in the last FLOOR_IDLE_MINUTES
/// stays: a reader may have it open.
///
/// The prune walks and measures the cache without the exclusive lock. It
/// takes that lock for each planned deletion, and stops if someone has
/// accessed the cache since planning began.
pub fn prune(
    tool: &Tool,
    run: Run,
    root: &Path,
    cap_kib: u64,
    groups: impl FnOnce() -> Result<Vec<Group>>,
) -> Result<()> {
    prune_with(tool, run, root, cap_kib, groups, HeldPaths::read)
}

fn prune_with(
    tool: &Tool,
    run: Run,
    root: &Path,
    cap_kib: u64,
    groups: impl FnOnce() -> Result<Vec<Group>>,
    open_files: impl FnOnce() -> Result<HeldPaths>,
) -> Result<()> {
    let Some(_pruning) = crate::evict::try_lock_creating(&root.join(PRUNING))? else {
        note(format!("Another prune is working in {}.", root.display()));
        return Ok(());
    };
    let Some(alone) = idle(root)? else {
        return Ok(());
    };
    let marker = root.join(PLAN);
    std::fs::File::create(&marker).context(|| format!("creating {}", marker.display()))?;
    drop(alone);

    let mut invalidated = BTreeMap::new();
    let deleted = plan(tool, root, cap_kib, groups, open_files)
        .and_then(|steps| apply_plan(tool, run, &marker, steps, &mut invalidated));
    let cleanup = remove_if_present(&marker);
    let changed = invalidated
        .into_iter()
        .filter_map(|(root, ready)| ready.then_some(root))
        .collect::<Vec<_>>();
    if !changed.is_empty() {
        crate::daemon::client::wrote(&tool.places, &changed);
    }
    with_cleanup(deleted, cleanup)
}

fn idle(root: &Path) -> Result<Option<std::fs::File>> {
    let lock = crate::evict::try_lock_creating(&root.join(WRITING))?;
    if lock.is_none() {
        note(format!(
            "A reader or writer holds {}. The prune skips it.",
            root.display()
        ));
    }
    Ok(lock)
}

fn plan_valid(marker: &Path) -> Result<bool> {
    let valid = marker
        .try_exists()
        .context(|| format!("checking {}", marker.display()))?;
    if !valid {
        note(format!(
            "A reader or writer used {} after the prune began preparing its deletions. The prune skips the remaining deletions.",
            marker
                .parent()
                .expect("a plan belongs to a cache")
                .display()
        ));
    }
    Ok(valid)
}

fn plan(
    tool: &Tool,
    root: &Path,
    cap_kib: u64,
    groups: impl FnOnce() -> Result<Vec<Group>>,
    open_files: impl FnOnce() -> Result<HeldPaths>,
) -> Result<Vec<Deletion>> {
    let staging = folders_in(root)?
        .into_iter()
        .filter(|(name, _)| is_staging(name))
        .map(|(_, folder)| folder)
        .collect::<Vec<_>>();
    let groups = groups()?
        .into_iter()
        .map(|group| measure_group(tool, cap_kib, group))
        .collect::<Result<Vec<_>>>()?;
    let recent = tool.now - (FLOOR_IDLE_MINUTES * 60) as i64;
    // Every group has been measured. Read the processes only if selection
    // reaches a folder, as close to the deletion phase as possible.
    let mut open = OpenFiles {
        read: Some(open_files),
        held: None,
    };
    let mut steps = Vec::new();
    for folder in staging {
        let unit = Unit::Folder(folder);
        let reason = "the staging folder of a writer that ended".to_string();
        if open.available(&unit, &reason)? {
            steps.push(Deletion { unit, reason });
        }
    }
    for group in groups {
        group.choose(cap_kib, recent, &mut open, &mut steps)?;
    }
    Ok(steps)
}

fn apply_plan(
    tool: &Tool,
    run: Run,
    marker: &Path,
    steps: Vec<Deletion>,
    invalidated: &mut BTreeMap<PathBuf, bool>,
) -> Result<()> {
    let root = marker.parent().expect("a plan belongs to a cache");
    if run.dry() {
        let Some(alone) = idle(root)? else {
            return Ok(());
        };
        if !plan_valid(marker)? {
            return Ok(());
        }
        drop(alone);
        for step in steps {
            note(format!(
                "would delete {} ({})",
                step.unit.path().display(),
                step.reason
            ));
        }
        return Ok(());
    }
    if steps.is_empty() {
        return Ok(());
    }
    let slot = crate::trash::Slot::make(&tool.places)?;
    let deleted = apply_steps(&tool.places, root, marker, steps, &slot, invalidated);
    with_cleanup(deleted, slot.remove_background())
}

fn apply_steps(
    places: &crate::places::Places,
    root: &Path,
    marker: &Path,
    steps: Vec<Deletion>,
    trash: &crate::trash::Slot,
    invalidated: &mut BTreeMap<PathBuf, bool>,
) -> Result<()> {
    let mut failures = Ok(());
    for (index, Deletion { unit, reason }) in steps.into_iter().enumerate() {
        let Some(alone) = idle(root)? else {
            break;
        };
        if !plan_valid(marker)? {
            break;
        }
        let path = unit.path();
        if let Some(changed_root) = crate::sizes::root_of(places, path) {
            let ready = invalidated.entry(changed_root.clone()).or_insert_with(|| {
                match crate::sizes::invalidate_root(places, &changed_root) {
                    Ok(()) => true,
                    Err(error) => {
                        failures =
                            with_cleanup(std::mem::replace(&mut failures, Ok(())), Err(error));
                        false
                    }
                }
            });
            if !*ready {
                continue;
            }
        }
        let moved = unit.move_to(trash, index).and_then(|moved| {
            if moved {
                remove_empty_parents(root, path)?;
            }
            Ok(moved)
        });
        drop(alone);
        match moved {
            Ok(true) => note(format!("deleted {} ({reason})", path.display())),
            Ok(false) => {}
            Err(error) => {
                failures = with_cleanup(failures, Err(error));
            }
        }
    }
    failures
}

impl Unit {
    fn path(&self) -> &Path {
        match self {
            Self::Folder(folder) => folder,
            Self::Files(files) => &files[0],
        }
    }

    /// Move the picture before its companion list. A failure can leave a
    /// list without its picture, which no reader uses.
    fn move_to(&self, trash: &crate::trash::Slot, index: usize) -> Result<bool> {
        let paths = match self {
            Self::Folder(folder) => std::slice::from_ref(folder),
            Self::Files(files) => files.as_slice(),
        };
        let mut moved = false;
        for (part, path) in paths.iter().enumerate() {
            let name = format!("{index}-{part}");
            moved |= crate::evict::move_to_slot(path, &trash.path, std::ffi::OsStr::new(&name))?;
        }
        Ok(moved)
    }
}

struct OpenFiles<F> {
    read: Option<F>,
    held: Option<HeldPaths>,
}

impl<F: FnOnce() -> Result<HeldPaths>> OpenFiles<F> {
    fn available(&mut self, unit: &Unit, reason: &str) -> Result<bool> {
        let Unit::Folder(folder) = unit else {
            return Ok(true);
        };
        let folder = match std::fs::canonicalize(folder) {
            Ok(folder) => folder,
            Err(error) if fail::gone(&error) => {
                note(format!(
                    "{} disappeared before the prune could delete it",
                    folder.display()
                ));
                return Ok(false);
            }
            Err(error) => fail!("resolving {}: {error}", folder.display()),
        };
        let held = match &self.held {
            Some(held) => held,
            None => self
                .held
                .insert(self.read.take().expect("one read supplies the snapshot")()?),
        };
        if held.holds(&folder) {
            note(format!("in use, kept: {} ({reason})", folder.display()));
            return Ok(false);
        }
        Ok(true)
    }
}

/// Measures all units before the process check, outside `WRITING`.
fn measure_group(tool: &Tool, cap_kib: u64, group: Group) -> Result<MeasuredGroup> {
    let mut paths = vec![group.folder.clone()];
    for kept in &group.kept {
        match kept {
            Kept::Folder { folder, .. } => paths.push(folder.path.clone()),
            Kept::InUse { folder, .. } | Kept::Staging { folder } => paths.push(folder.clone()),
            Kept::Files { .. } | Kept::Loose { .. } => {}
        }
    }
    let mut measures = tool.measure_all(&paths)?.into_iter();
    let total = measures.next().expect("the folder was measured").kib;
    let mut abandoned = Vec::new();
    let mut units = Vec::new();
    let mut in_use: (u64, usize, BTreeSet<String>) = Default::default();
    for kept in group.kept {
        match kept {
            Kept::InUse { by, .. } => {
                let measured = measures.next().expect("the folder was measured");
                in_use.0 += measured.kib;
                in_use.1 += 1;
                in_use.2.extend(by);
            }
            Kept::Staging { folder } => {
                let measured = measures.next().expect("the folder was measured");
                let reason = format!(
                    "{}, the staging folder of a writer that ended",
                    human(measured.kib)
                );
                abandoned.push((
                    Deletion {
                        unit: Unit::Folder(folder),
                        reason,
                    },
                    measured.kib,
                ));
            }
            Kept::Loose { file } => {
                let kib = file_kib(&file)?;
                let reason = format!(
                    "{}, a half-written file from a writer that ended",
                    human(kib)
                );
                abandoned.push((
                    Deletion {
                        unit: Unit::Files(vec![file]),
                        reason,
                    },
                    kib,
                ));
            }
            Kept::Folder {
                folder,
                last_use,
                what,
            } => {
                let measured = measures.next().expect("the folder was measured");
                let used = match last_use {
                    Use::At(time) => time,
                    Use::NewestInside => {
                        let Some(newest) = measured.newest else {
                            note(format!(
                                "{} disappeared during prune planning",
                                folder.path.display()
                            ));
                            continue;
                        };
                        newest
                    }
                };
                units.push(MeasuredUnit {
                    used,
                    kib: measured.kib,
                    what,
                    unit: Unit::Folder(folder),
                });
            }
            Kept::Files {
                files,
                last_use,
                what,
            } => {
                let mut kib = 0;
                for file in &files {
                    kib += file_kib(file)?;
                }
                units.push(MeasuredUnit {
                    used: last_use,
                    kib,
                    what,
                    unit: Unit::Files(files),
                });
            }
        }
    }
    units.sort_by_key(|unit| unit.used);
    let (in_use_kib, count, by) = in_use;
    if in_use_kib > cap_kib {
        note(format!(
            "{} holds {} of {count} units that {} still need, over its {} cap; kept",
            group.folder.display(),
            human(in_use_kib),
            by.into_iter().collect::<Vec<_>>().join(", "),
            human(cap_kib)
        ));
    }
    Ok(MeasuredGroup {
        folder: group.folder,
        total,
        abandoned,
        units,
    })
}

impl MeasuredGroup {
    fn choose(
        self,
        cap_kib: u64,
        recent: i64,
        open: &mut OpenFiles<impl FnOnce() -> Result<HeldPaths>>,
        steps: &mut Vec<Deletion>,
    ) -> Result<()> {
        let mut total = self.total;
        for (step, kib) in self.abandoned {
            if open.available(&step.unit, &step.reason)? {
                total = total.saturating_sub(kib);
                steps.push(step);
            }
        }
        for MeasuredUnit {
            used,
            kib,
            what,
            unit,
        } in self.units
        {
            if total <= cap_kib || used >= recent {
                break;
            }
            let reason = format!(
                "{}, {what}; {} is {} > {}",
                human(kib),
                self.folder.display(),
                human(total),
                human(cap_kib)
            );
            if open.available(&unit, &reason)? {
                total = total.saturating_sub(kib);
                steps.push(Deletion { unit, reason });
            }
        }
        Ok(())
    }
}

/// The modification time a unit uses, or none after its file disappeared.
pub(super) fn written_at(path: &Path) -> Result<Option<i64>> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.mtime())),
        Err(error) if fail::gone(&error) => {
            note(format!(
                "{} disappeared during prune planning",
                path.display()
            ));
            Ok(None)
        }
        Err(error) => fail!("reading the time of {}: {error}", path.display()),
    }
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

/// Removes only the ancestors a deletion could have emptied. The caller
/// holds `WRITING` exclusively, so no writer is about to populate them.
fn remove_empty_parents(root: &Path, deleted: &Path) -> Result<()> {
    for parent in deleted
        .ancestors()
        .skip(1)
        .take_while(|parent| *parent != root)
    {
        match std::fs::remove_dir(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) => fail!("removing {}: {error}", parent.display()),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "capped_store_tests.rs"]
mod tests;

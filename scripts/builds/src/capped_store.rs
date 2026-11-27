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
//! One unit may be thousands of files, so the prune says what it deleted in
//! one line for each group's folder.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::evict::HeldPaths;
use crate::fail;
use crate::fail::{Context, Result, with_move_cleanup};
use crate::output::note;
use crate::places::FLOOR_IDLE_MINUTES;
use crate::prune::Run;
use crate::text::{counted, human};
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
    let shared = crate::flock::lock_creating(&root.join(WRITING), libc::LOCK_SH)?;
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

/// When a published unit was last used, as `published_use` reads it.
pub enum Published {
    /// At a time, seconds since 1970.
    Used(i64),
    /// The unit is gone.
    Gone,
    /// A reader or writer used the root after the prune began its plan, which
    /// the prune then drops.
    PlanLost,
}

/// When the unit <folder> of <root> was last used: when its file <file> was
/// written. A published unit holds the file, so a unit without it is an
/// error that names it as a published <what>.
pub fn published_use(root: &Path, folder: &Path, file: &str, what: &str) -> Result<Published> {
    let used = folder.join(file);
    if let Some(at) = written_at(&used)? {
        return Ok(Published::Used(at));
    }
    // Publishing removes the plan marker before changing any files. A valid
    // plan and the exclusive lock rule out a replacement between the missing
    // file and this diagnosis.
    let Some(_valid) = lock_plan(root)? else {
        return Ok(Published::PlanLost);
    };
    if folder
        .try_exists()
        .context(|| format!("checking {}", folder.display()))?
    {
        fail!("The published {what} is missing {}.", used.display());
    }
    note(format!(
        "{} disappeared during prune planning",
        folder.display()
    ));
    Ok(Published::Gone)
}

/// The folder a capped unit occupies and the lock its readers hold, if any.
pub struct Folder {
    pub path: PathBuf,
    /// Readers acquire this lock while holding WRITING.
    pub reader_lock: Option<PathBuf>,
}

impl Folder {
    pub(super) fn unpinned(path: PathBuf) -> Self {
        Self {
            path,
            reader_lock: None,
        }
    }
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
    kib: u64,
    /// Why this unit goes, which the line that keeps a unit in use gives.
    reason: String,
}

/// What the prune deletes in one folder.
#[derive(Default)]
struct FolderPlan {
    /// What the folder holds and its cap, when the cap picks a unit.
    over_cap: Option<OverCapBy>,
    steps: Vec<Deletion>,
}

#[derive(Clone, Copy)]
struct OverCapBy {
    held_kib: u64,
    cap_kib: u64,
}

/// What the prune deletes, by the folder whose line reports it.
type Plan = BTreeMap<PathBuf, FolderPlan>;

struct MeasuredUnit {
    used: i64,
    kib: u64,
    what: &'static str,
    unit: Unit,
}

struct MeasuredGroup {
    folder: PathBuf,
    total: u64,
    abandoned: Vec<Deletion>,
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
    groups: impl FnOnce() -> Result<Option<Vec<Group>>>,
) -> Result<()> {
    prune_with(tool, run, root, cap_kib, groups, || {
        HeldPaths::read(&tool.system)
    })
}

fn prune_with(
    tool: &Tool,
    run: Run,
    root: &Path,
    cap_kib: u64,
    groups: impl FnOnce() -> Result<Option<Vec<Group>>>,
    open_files: impl Fn() -> Result<HeldPaths>,
) -> Result<()> {
    let Some(_pruning) = crate::flock::try_lock_creating(&root.join(PRUNING), libc::LOCK_EX)?
    else {
        note(format!("Another prune is working in {}.", root.display()));
        return Ok(());
    };
    let Some(alone) = idle(root)? else {
        return Ok(());
    };
    let marker = root.join(PLAN);
    std::fs::File::create(&marker).context(|| format!("creating {}", marker.display()))?;
    drop(alone);

    let deleted = plan(tool, root, cap_kib, groups, open_files)
        .and_then(|steps| apply_plan(tool, run, root, steps));
    with_move_cleanup(deleted, remove_if_present(&marker).map(drop))
}

fn idle(root: &Path) -> Result<Option<std::fs::File>> {
    let lock = crate::flock::try_lock_creating(&root.join(WRITING), libc::LOCK_EX)?;
    if lock.is_none() {
        note(format!(
            "A reader or writer holds {}. The prune skips it.",
            root.display()
        ));
    }
    Ok(lock)
}

fn plan_current(root: &Path) -> Result<bool> {
    let marker = root.join(PLAN);
    marker
        .try_exists()
        .context(|| format!("checking {}", marker.display()))
}

fn note_plan_invalid(root: &Path) {
    note(format!(
        "A reader or writer used {} after the prune began preparing its deletions. The prune skips the remaining deletions.",
        root.display()
    ));
}

fn plan(
    tool: &Tool,
    root: &Path,
    cap_kib: u64,
    groups: impl FnOnce() -> Result<Option<Vec<Group>>>,
    open_files: impl Fn() -> Result<HeldPaths>,
) -> Result<Plan> {
    let staging = folders_in(root)?
        .into_iter()
        .filter(|(name, _)| is_staging(name))
        .map(|(_, folder)| folder)
        .collect::<Vec<_>>();
    let Some(groups) = groups()? else {
        return Ok(Plan::new());
    };
    if !plan_current(root)? {
        note_plan_invalid(root);
        return Ok(Plan::new());
    }
    let staging_sizes = tool.measure_all(&staging)?;
    let groups = groups
        .into_iter()
        .map(|group| measure_group(tool, cap_kib, group))
        .collect::<Result<Vec<_>>>()?;
    let recent = tool.now - (FLOOR_IDLE_MINUTES * 60) as i64;
    // Every group has been measured. Read the processes only if selection
    // reaches a folder, as close to the deletion phase as possible.
    let mut open = OpenFiles::Unread(open_files);
    let mut plan = Plan::new();
    for (folder, measured) in staging.into_iter().zip(staging_sizes) {
        let unit = Unit::Folder(Folder::unpinned(folder));
        let reason = "the staging folder of a writer that ended".to_string();
        if open.available(&unit, &reason)? {
            plan.entry(root.to_path_buf())
                .or_default()
                .steps
                .push(Deletion {
                    unit,
                    kib: measured.kib,
                    reason,
                });
        }
    }
    for group in groups {
        group.choose(cap_kib, recent, &mut open, &mut plan)?;
    }
    Ok(plan)
}

/// Takes the cache's exclusive lock if the current prune still owns a valid plan.
pub(super) fn lock_plan(root: &Path) -> Result<Option<std::fs::File>> {
    let Some(alone) = idle(root)? else {
        return Ok(None);
    };
    if plan_current(root)? {
        return Ok(Some(alone));
    }
    drop(alone);
    note_plan_invalid(root);
    Ok(None)
}

fn apply_plan(tool: &Tool, run: Run, root: &Path, plan: Plan) -> Result<()> {
    if plan.is_empty() {
        return Ok(());
    }
    if run.dry() {
        let Some(alone) = lock_plan(root)? else {
            return Ok(());
        };
        drop(alone);
        for (folder, planned) in plan {
            let mut cleaned = Cleaned::default();
            for step in &planned.steps {
                match &step.unit {
                    Unit::Folder(_) => cleaned.folders += 1,
                    Unit::Files(files) => cleaned.files += files.len(),
                }
                cleaned.kib += step.kib;
            }
            cleaned.report(run, &folder, planned.over_cap);
        }
        return Ok(());
    }
    let mut applying = Applying {
        tool,
        root,
        trash: None,
        tried: 0,
        failures: Ok(()),
    };
    for (folder, planned) in plan {
        let mut cleaned = Cleaned::default();
        let went_on = planned
            .steps
            .into_iter()
            .try_for_each(|step| applying.delete(step, &mut cleaned));
        cleaned.report(run, &folder, planned.over_cap);
        if went_on.is_break() {
            break;
        }
    }
    match applying.trash {
        Some(slot) => with_move_cleanup(applying.failures, slot.remove_background()),
        None => applying.failures,
    }
}

/// What a prune deleted in one folder, or on a dry run would delete.
#[derive(Default)]
struct Cleaned {
    folders: usize,
    files: usize,
    kib: u64,
    /// The planned entries something else deleted first.
    gone: usize,
}

impl Cleaned {
    /// Says the folder's line, and one for the entries something else
    /// deleted first.
    fn report(&self, run: Run, folder: &Path, over_cap: Option<OverCapBy>) {
        let counts: Vec<String> = [
            (self.folders, "folder", "folders"),
            (self.files, "file", "files"),
        ]
        .into_iter()
        .filter(|(count, _, _)| *count > 0)
        .map(|(count, one, many)| counted(count, one, many))
        .collect();
        if !counts.is_empty() {
            let freed = if run.dry() {
                "which would free"
            } else {
                "which freed"
            };
            let why = over_cap.map_or_else(String::new, |over| {
                format!(
                    " (the folder held {}, over its {} cap)",
                    human(over.held_kib),
                    human(over.cap_kib)
                )
            });
            note(format!(
                "{} {} in {}, {freed} {}{why}",
                run.deleted(),
                counts.join(" and "),
                folder.display(),
                human(self.kib)
            ));
        }
        if self.gone > 0 {
            note(format!(
                "something else deleted {} in {} before the prune did",
                counted(self.gone, "entry", "entries"),
                folder.display()
            ));
        }
    }
}

/// The deletions of one prune as it carries them out.
struct Applying<'a> {
    tool: &'a Tool,
    root: &'a Path,
    trash: Option<crate::trash::Slot>,
    /// How many units the prune has started to delete, which names each
    /// folder's place in the trash slot.
    tried: usize,
    failures: Result<()>,
}

impl Applying<'_> {
    /// Deletes <step>'s unit and counts what went in <cleaned>. Breaks when
    /// the prune must delete nothing more.
    fn delete(&mut self, step: Deletion, cleaned: &mut Cleaned) -> ControlFlow<()> {
        let Deletion { unit, kib, .. } = step;
        let root = self.root;
        let target = match &unit {
            Unit::Folder(folder) => {
                let slot = match &self.trash {
                    Some(slot) => slot,
                    None => {
                        // The trash lock can wait for an emptier. Check the
                        // plan first, then release the cache lock to get a slot.
                        match lock_plan(root) {
                            Ok(Some(alone)) => drop(alone),
                            Ok(None) => return ControlFlow::Break(()),
                            Err(error) => return self.stop(error),
                        }
                        match crate::trash::Slot::make(&self.tool.places) {
                            Ok(slot) => self.trash.insert(slot),
                            Err(error) => return self.stop(error),
                        }
                    }
                };
                Target::Folder {
                    path: &folder.path,
                    slot,
                }
            }
            Unit::Files(files) => Target::Files(files),
        };
        let alone = match lock_plan(root) {
            Ok(Some(alone)) => alone,
            Ok(None) => return ControlFlow::Break(()),
            Err(error) => return self.stop(error),
        };
        let mut removed = Vec::new();
        let unlinked = target.delete(self.tried, &mut removed);
        self.tried += 1;
        let any = removed.contains(&true);
        if unlinked.is_ok() && any {
            cleaned.kib += kib;
        }
        let deleted = unlinked.and_then(|()| {
            if any {
                remove_empty_parents(root, unit.path())?;
            }
            Ok(())
        });
        drop(alone);
        for removed in removed {
            match (removed, &unit) {
                (true, Unit::Folder(_)) => cleaned.folders += 1,
                (true, Unit::Files(_)) => cleaned.files += 1,
                (false, _) => cleaned.gone += 1,
            }
        }
        self.keep(deleted);
        ControlFlow::Continue(())
    }

    /// Adds <result>'s failure to the ones the prune returns.
    fn keep(&mut self, result: Result<()>) {
        let earlier = std::mem::replace(&mut self.failures, Ok(()));
        self.failures = with_move_cleanup(earlier, result);
    }

    fn stop(&mut self, error: fail::Fail) -> ControlFlow<()> {
        self.keep(Err(error));
        ControlFlow::Break(())
    }
}

fn note_disappeared(path: &Path) {
    note(format!(
        "{} disappeared before the prune could delete it",
        path.display()
    ));
}

impl Unit {
    fn path(&self) -> &Path {
        match self {
            Self::Folder(folder) => &folder.path,
            Self::Files(files) => &files[0],
        }
    }
}

/// Each folder deletion carries the slot that owns its destination. File
/// units unlink their files in order and do not need a trash slot.
enum Target<'a> {
    Folder {
        path: &'a Path,
        slot: &'a crate::trash::Slot,
    },
    Files(&'a [PathBuf]),
}

impl Target<'_> {
    /// Deletes the unit's paths in order, adding to <removed> whether each
    /// one was still there.
    fn delete(self, index: usize, removed: &mut Vec<bool>) -> Result<()> {
        match self {
            Self::Folder { path, slot } => {
                removed.push(crate::evict::move_to_slot(
                    path,
                    &slot.path,
                    std::ffi::OsStr::new(&index.to_string()),
                )?);
            }
            Self::Files(files) => {
                for file in files {
                    removed.push(remove_if_present(file)?);
                }
            }
        }
        Ok(())
    }
}

enum OpenFiles<F> {
    Unread(F),
    Read(HeldPaths),
}

impl<F: Fn() -> Result<HeldPaths>> OpenFiles<F> {
    fn available(&mut self, unit: &Unit, reason: &str) -> Result<bool> {
        let Unit::Folder(folder) = unit else {
            return Ok(true);
        };
        let path = match std::fs::canonicalize(&folder.path) {
            Ok(folder) => folder,
            Err(error) if fail::gone(&error) => {
                note_disappeared(&folder.path);
                return Ok(false);
            }
            Err(error) => fail!("resolving {}: {error}", folder.path.display()),
        };
        // A reader takes its pin through WRITING, which invalidates the
        // plan. The final lock_plan check excludes readers that arrive later.
        let pinned = match &folder.reader_lock {
            Some(lock) => crate::flock::is_held(lock)?,
            None => false,
        };
        let held = pinned
            || match self {
                Self::Unread(read) => {
                    let held = read()?;
                    let used = held.holds(&path);
                    *self = Self::Read(held);
                    used
                }
                Self::Read(held) => held.holds(&path),
            };
        if held {
            note(format!("in use, kept: {} ({reason})", path.display()));
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
                abandoned.push(Deletion {
                    unit: Unit::Folder(Folder::unpinned(folder)),
                    kib: measured.kib,
                    reason,
                });
            }
            Kept::Loose { file } => {
                let kib = file_kib(&file)?;
                let reason = format!(
                    "{}, a half-written file from a writer that ended",
                    human(kib)
                );
                abandoned.push(Deletion {
                    unit: Unit::Files(vec![file]),
                    kib,
                    reason,
                });
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
        open: &mut OpenFiles<impl Fn() -> Result<HeldPaths>>,
        plan: &mut Plan,
    ) -> Result<()> {
        let mut total = self.total;
        let mut steps = Vec::new();
        let mut over_cap = None;
        for step in self.abandoned {
            if open.available(&step.unit, &step.reason)? {
                total = total.saturating_sub(step.kib);
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
                over_cap = Some(OverCapBy {
                    held_kib: self.total,
                    cap_kib,
                });
                steps.push(Deletion { unit, kib, reason });
            }
        }
        if !steps.is_empty() {
            let folder = plan.entry(self.folder).or_default();
            folder.over_cap = folder.over_cap.or(over_cap);
            folder.steps.extend(steps);
        }
        Ok(())
    }
}

/// The modification time of a regular file, or none if its path is absent.
pub(super) fn written_at(path: &Path) -> Result<Option<i64>> {
    use std::os::unix::fs::MetadataExt;
    match crate::whole_file::stat(path)? {
        Some(meta) if meta.is_file() => Ok(Some(meta.mtime())),
        Some(_) => fail!("{} must be a file", path.display()),
        None => Ok(None),
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

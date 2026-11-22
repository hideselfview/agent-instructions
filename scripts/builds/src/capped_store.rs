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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::evict::Recreate;
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::FLOOR_IDLE_MINUTES;
use crate::prune::Run;
use crate::text::{human, note};
use crate::tool::Tool;

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
    remove_file(&root.join(PLAN))?;
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
    /// A unit that is several files, which the prune deletes in order.
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

    let planned = plan(tool, root, cap_kib, groups);
    let mut changed = BTreeSet::new();
    let deleted = apply_plan(tool, run, root, &marker, planned, &mut changed);
    let cleanup = remove_file(&marker);
    let errors = [deleted, cleanup]
        .into_iter()
        .chain(
            changed
                .iter()
                .map(|folder| crate::sizes::stale(&tool.places, folder)),
        )
        .filter_map(Result::err)
        .map(crate::fail::Fail::text)
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return Ok(());
    }
    fail!("{}", errors.join("; "))
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
            "A reader or writer used {} during planning. The prune skips the remaining deletions.",
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
) -> Result<Vec<Deletion>> {
    let mut steps = Vec::new();
    let mut held = None;
    for (name, folder) in folders_in(root)? {
        if is_staging(&name) {
            let reason = "the staging folder of a writer that ended".to_string();
            if folder_available(&folder, &reason, &mut held)? {
                steps.push(Deletion {
                    unit: Unit::Folder(folder),
                    reason,
                });
            }
        }
    }
    for group in groups()? {
        steps.extend(plan_group(tool, cap_kib, group, &mut held)?);
    }
    Ok(steps)
}

fn apply_plan(
    tool: &Tool,
    run: Run,
    root: &Path,
    marker: &Path,
    planned: Result<Vec<Deletion>>,
    changed: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let steps = match planned {
        Ok(steps) => steps,
        Err(error) => {
            // Concurrent access can invalidate a path while it is measured.
            let Some(_alone) = idle(root)? else {
                return Ok(());
            };
            return if plan_valid(marker)? {
                Err(error)
            } else {
                Ok(())
            };
        }
    };
    if steps.is_empty() {
        return Ok(());
    }
    let trash = if run.dry() {
        None
    } else {
        let hold = crate::trash::lock_shared(&tool.places)?;
        Some((hold, crate::trash::Slot::make(&tool.places)?))
    };
    let deleted = apply_steps(
        &tool.places,
        marker,
        steps,
        trash.as_ref().map(|(_, slot)| slot),
        changed,
    );
    let removed = match trash {
        Some((hold, slot)) => {
            let removed = slot.remove_background();
            drop(hold);
            removed
        }
        None => Ok(()),
    };
    match (deleted, removed) {
        (Err(error), Err(also)) => fail!("{}; {}", error.text(), also.text()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn apply_steps(
    places: &crate::places::Places,
    marker: &Path,
    steps: Vec<Deletion>,
    trash: Option<&crate::trash::Slot>,
    changed: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let root = marker.parent().expect("a plan belongs to a cache");
    for (index, Deletion { unit, reason }) in steps.into_iter().enumerate() {
        let Some(alone) = idle(root)? else {
            break;
        };
        if !plan_valid(marker)? {
            break;
        }
        let shown = match &unit {
            Unit::Folder(folder) => folder,
            Unit::Files(files) => &files[0],
        };
        let Some(slot) = trash else {
            drop(alone);
            note(format!("would delete {} ({reason})", shown.display()));
            continue;
        };
        let moved = match &unit {
            Unit::Folder(folder) => {
                let moved = crate::evict::move_in(
                    folder,
                    &slot.path,
                    std::ffi::OsStr::new(&index.to_string()),
                    Recreate::No,
                )?;
                if moved {
                    if let Some(changed_root) = crate::sizes::root_of(places, folder) {
                        changed.insert(changed_root);
                    }
                    remove_empty_parents(root, folder)?;
                }
                moved
            }
            Unit::Files(files) => {
                for file in files {
                    remove_file(file)?;
                }
                for file in files {
                    remove_empty_parents(root, file)?;
                }
                true
            }
        };
        drop(alone);
        if moved {
            note(format!("deleted {} ({reason})", shown.display()));
        }
    }
    Ok(())
}

fn folder_available(
    folder: &Path,
    reason: &str,
    held: &mut Option<crate::evict::HeldPaths>,
) -> Result<bool> {
    let open = match held {
        Some(open) => open,
        None => held.insert(crate::evict::HeldPaths::read()?),
    };
    let folder =
        std::fs::canonicalize(folder).context(|| format!("resolving {}", folder.display()))?;
    if open.holds(&folder) {
        note(format!("in use, kept: {} ({reason})", folder.display()));
        return Ok(false);
    }
    Ok(true)
}

/// What `prune` deletes in one group, in order, read without holding the
/// root: the staging folders and loose files, then the least recently used
/// units, while the group is over <cap_kib>. A unit a process holds open (one
/// reading of the open files) stays and counts as kept.
fn plan_group(
    tool: &Tool,
    cap_kib: u64,
    group: Group,
    held: &mut Option<crate::evict::HeldPaths>,
) -> Result<Vec<Deletion>> {
    let mut steps = Vec::new();
    let mut paths = vec![group.folder.clone()];
    for kept in &group.kept {
        match kept {
            Kept::Folder { folder, .. } => paths.push(folder.path.clone()),
            Kept::InUse { folder, .. } | Kept::Staging { folder } => paths.push(folder.clone()),
            Kept::Files { .. } | Kept::Loose { .. } => {}
        }
    }
    let mut measures = tool.measure_all(&paths)?.into_iter();
    let mut total = measures.next().expect("the folder was measured").kib;
    let mut units: Vec<(i64, u64, &'static str, Unit)> = Vec::new();
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
                if folder_available(&folder, &reason, held)? {
                    total = total.saturating_sub(measured.kib);
                    steps.push(Deletion {
                        unit: Unit::Folder(folder),
                        reason,
                    });
                }
            }
            Kept::Loose { file } => {
                let kib = file_kib(&file)?;
                let reason = format!(
                    "{}, a file that a writer which ended left half written",
                    human(kib)
                );
                total = total.saturating_sub(kib);
                steps.push(Deletion {
                    unit: Unit::Files(vec![file]),
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
                    Use::NewestInside => measured.newest.ok_or_else(|| {
                        fail::Fail::Message(format!(
                            "{} disappeared during prune planning",
                            folder.path.display()
                        ))
                    })?,
                };
                units.push((used, measured.kib, what, Unit::Folder(folder)));
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
                units.push((last_use, kib, what, Unit::Files(files)));
            }
        }
    }
    units.sort_by_key(|unit| unit.0);
    let recent = tool.now - (FLOOR_IDLE_MINUTES * 60) as i64;
    for (used, kib, what, unit) in units {
        if total <= cap_kib {
            break;
        }
        if used >= recent {
            continue;
        }
        let reason = format!(
            "{}, {what}; {} is {} > {}",
            human(kib),
            group.folder.display(),
            human(total),
            human(cap_kib)
        );
        if let Unit::Folder(folder) = &unit
            && !folder_available(folder, &reason, held)?
        {
            continue;
        }
        total = total.saturating_sub(kib);
        steps.push(Deletion { unit, reason });
    }
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
    Ok(steps)
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

/// Removes the file if it is present.
fn remove_file(file: &Path) -> Result<()> {
    match std::fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => fail!("deleting {}: {error}", file.display()),
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

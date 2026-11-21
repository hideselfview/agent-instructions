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

use crate::evict::{Recreate, Removal};
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::FLOOR_IDLE_MINUTES;
use crate::prune::{Run, delete_folder};
use crate::text::{human, note};
use crate::tool::Tool;

/// The lock file in a root that each writer holds shared and the prune
/// exclusively while it deletes staging folders and empty folders.
const WRITING: &str = ".writing";
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
    crate::evict::lock_shared_creating(&root.join(WRITING))
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

/// What a group of a root holds, as the cap sees it.
pub enum Kept {
    /// A unit that is one folder; <what> names it in the reason its deletion
    /// gives.
    Folder {
        folder: PathBuf,
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
    Folder(PathBuf),
    Files(Vec<PathBuf>),
}

/// Deletes nothing while a writer or reader holds <root>; else each staging
/// folder at the top of <root> and, in each of <groups>, each staging folder
/// and loose file (the writers that made them ended), then, while the group's folder holds more than <cap_kib>, whole
/// units, least recently used first, and last the folders under <root> that
/// the deletions left empty. A unit used in the last FLOOR_IDLE_MINUTES
/// stays: a reader may have it open.
pub fn prune(tool: &Tool, run: Run, root: &Path, cap_kib: u64, groups: Vec<Group>) -> Result<()> {
    // Held to the end: nobody reads or writes the root meanwhile.
    let Some(_alone) = crate::evict::try_lock_creating(&root.join(WRITING))? else {
        note(format!(
            "a writer or reader holds {}, so prune deletes nothing in it",
            root.display()
        ));
        return Ok(());
    };
    for (name, folder) in folders_in(root)? {
        if !is_staging(&name) {
            continue;
        }
        delete_folder(
            tool,
            &folder,
            run,
            "the staging folder of a writer that ended",
            Recreate::No,
            Removal::Background,
            "",
        )?;
    }
    for group in groups {
        prune_group(tool, run, cap_kib, group)?;
    }
    if !run.dry() {
        remove_empty(root)?;
    }
    Ok(())
}

/// `prune` for one group.
fn prune_group(tool: &Tool, run: Run, cap_kib: u64, group: Group) -> Result<()> {
    let mut paths = vec![group.folder.clone()];
    for kept in &group.kept {
        match kept {
            Kept::Folder { folder, .. } | Kept::InUse { folder, .. } | Kept::Staging { folder } => {
                paths.push(folder.clone())
            }
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
                if delete_folder(
                    tool,
                    &folder,
                    run,
                    &reason,
                    Recreate::No,
                    Removal::Background,
                    "",
                )? {
                    total = total.saturating_sub(measured.kib);
                }
            }
            Kept::Loose { file } => {
                let kib = file_kib(&file)?;
                let reason = format!(
                    "{}, a file that a writer which ended left half written",
                    human(kib)
                );
                if delete_files(run, &[&file], &reason)? {
                    total = total.saturating_sub(kib);
                }
            }
            Kept::Folder {
                folder,
                last_use,
                what,
            } => {
                let measured = measures.next().expect("the folder was measured");
                let used = match last_use {
                    Use::At(time) => time,
                    // None: the folder went while the prune held the root
                    // exclusively, and only the prune deletes it.
                    Use::NewestInside => measured.newest.ok_or_else(|| {
                        fail::Fail::Message(format!(
                            "{} went while the prune held {WRITING} exclusively, and only the prune deletes it",
                            folder.display()
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
        let deleted = match &unit {
            Unit::Folder(folder) => delete_folder(
                tool,
                folder,
                run,
                &reason,
                Recreate::No,
                Removal::Background,
                "",
            )?,
            Unit::Files(files) => delete_files(run, &files.iter().collect::<Vec<_>>(), &reason)?,
        };
        if deleted {
            total = total.saturating_sub(kib);
        }
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
    Ok(())
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

/// Deletes <files> in order for <reason>, or says it would; returns whether
/// they went, or on a dry run would go.
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

/// Removes each folder under <root> that holds nothing, deepest first, and
/// leaves hidden folders alone. The caller holds `WRITING` exclusively.
fn remove_empty(root: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => fail!("listing {}: {error}", root.display()),
    };
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", root.display()))?;
        let path = entry.path();
        if entry.file_name().as_encoded_bytes().first() == Some(&b'.')
            || !path.is_dir()
            || path.is_symlink()
        {
            continue;
        }
        remove_empty(&path)?;
        match std::fs::remove_dir(&path) {
            Err(error)
                if !matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                fail!("removing {}: {error}", path.display())
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "capped_store_tests.rs"]
mod tests;

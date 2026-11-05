//! Bounding what builds leave behind: each checkout's unused build copies and
//! caps, DerivedData, the shared caches, build output in the temporary
//! folders, tests' temporary folders, Claude Code scratch folders, and cargo's
//! git checkouts. `setup` runs these for its checkout, `prune` for every one.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::checkout::{Checkout, Root, git_text, git, keeps_warm};
use crate::evict::{Recreate, Removal, background_rm, evict, make_temp_dir};
use crate::fail;
use crate::fail::{Context, Result};
use crate::manifest::{SHARED_CACHES, build_dirs};
use crate::places::{DERIVED_DATA_CAP_KIB, DERIVED_DATA_IDLE_MINUTES, FLOOR_IDLE_MINUTES};
use crate::temp_builds::temp_build_folders;
use crate::text::{human, note};
use crate::tool::Tool;
use crate::trees::prune_trees;
use crate::units;
use crate::usage::Measure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    Real,
    Dry,
}

impl Run {
    pub fn dry(self) -> bool {
        self == Run::Dry
    }
}

/// Deletes the unused build copies in the folders of <checkout>'s manifest in
/// <root>, then applies the manifest's caps to them; with <idle> minutes > 0,
/// also deletes folders nothing wrote to for that long.
pub fn prune_root(tool: &Tool, root: &Path, checkout: &Checkout, run: Run, idle: u64) -> Result<()> {
    let manifest = build_dirs(&checkout.top)?;
    let folders: Vec<PathBuf> =
        manifest.iter().map(|entry| root.join(&entry.path)).filter(|folder| folder.is_dir()).collect();
    if folders.is_empty() {
        return Ok(());
    }
    let ages = units::prune::Ages::new(
        tool.rules.unused_days,
        tool.rules.superseded_hours,
        tool.rules.incremental_hours,
    );
    let mut freed = units::prune::prune(&ages, run.dry(), &folders)?;
    freed.extend(prune_trees(tool, root, checkout, &folders, run)?);
    let claimed = checkout.claim_file().is_file();
    // Measured together, after the pruning: deleting one folder changes no
    // other's size.
    let measures = tool.measure_all(&folders)?;
    for entry in &manifest {
        let folder = root.join(&entry.path);
        let Some(at) = folders.iter().position(|f| *f == folder) else { continue };
        let measured = measures[at];
        let mut kib = measured.kib;
        if run.dry() {
            // Nothing was deleted; measure the folder as pruning would leave
            // it.
            let pruned: u64 = freed.iter().filter(|(f, _)| *f == folder).map(|(_, kib)| *kib).sum();
            kib = kib.saturating_sub(pruned);
        }
        let reason = if kib > entry.cap_kib {
            format!("{} > {}", human(kib), human(entry.cap_kib))
        } else if idle > 0 && kib > 1024 && !tool.written_within(&measured, idle) {
            format!("{}, no write in {idle}m", human(kib))
        } else {
            continue;
        };
        // A claimed worktree's agent is mid-task; emptying its folder costs it
        // a cold rebuild of every dependency, as the floor's exemption says.
        // Its incremental folders are pure cache, though, and an agent that
        // builds all day would otherwise grow the folder without bound: over
        // its cap, those go, least recently written first, until it fits.
        // Main's and the app checkout's folders are left whole.
        if claimed {
            if kib > entry.cap_kib && !keeps_warm(&tool.places, root)? {
                trim_claimed(tool, &folder, kib, entry.cap_kib, run, &reason)?;
            } else {
                note(format!("claimed, kept: {} ({reason})", folder.display()));
            }
            continue;
        }
        delete_folder(tool, &folder, run, &reason, Recreate::Yes, "; the next build refills it")?;
    }
    Ok(())
}

/// Deletes <folder> for <reason>, or says it would, or that it is in use.
fn delete_folder(tool: &Tool, folder: &Path, run: Run, reason: &str, recreate: Recreate, after: &str) -> Result<bool> {
    if run.dry() {
        note(format!("would delete {} ({reason})", folder.display()));
        Ok(false)
    } else if evict(&tool.places, folder, recreate, Removal::Background)? {
        note(format!("deleted {} ({reason}){after}", folder.display()));
        Ok(true)
    } else {
        note(format!("in use, kept: {} ({reason})", folder.display()));
        Ok(false)
    }
}

/// Trims the incremental folders of <folder>, a claimed worktree's cargo
/// folder of <kib> over its <cap>, least recently written first, until it is
/// under its cap or none is left; says what went and what is left.
fn trim_claimed(tool: &Tool, folder: &Path, kib: u64, cap: u64, run: Run, reason: &str) -> Result<()> {
    let freed = units::trim::trim(cap, kib, run.dry(), &[folder.to_path_buf()])?;
    let left = if run.dry() { kib.saturating_sub(freed) } else { tool.measure(folder)?.kib };
    if left > cap {
        note(format!(
            "claimed, kept: {} ({reason}; {} > {} after trimming its incremental folders)",
            folder.display(),
            human(left),
            human(cap)
        ));
    } else {
        note(format!(
            "claimed, {}: {} ({reason}; {} left, under the cap)",
            if run.dry() { "would trim" } else { "trimmed" },
            folder.display(),
            human(left)
        ));
    }
    Ok(())
}

/// A DerivedData entry and what it takes.
pub struct Entry {
    pub path: PathBuf,
    pub kib: u64,
    pub newest: i64,
}

/// DerivedData as a whole, and its entries, least recently written first,
/// measured together.
pub fn derived_data(tool: &Tool) -> Result<(Measure, Vec<Entry>)> {
    let dir = tool.places.derived_data();
    let mut paths = vec![dir.clone()];
    for name in crate::text::glob_names(&dir)? {
        paths.push(dir.join(name));
    }
    let mut measures = tool.measure_all(&paths)?.into_iter();
    let total = measures.next().expect("the folder was measured");
    let mut entries: Vec<Entry> = paths
        .into_iter()
        .skip(1)
        .zip(measures)
        .map(|(path, measured)| Entry { path, kib: measured.kib, newest: measured.newest.unwrap_or(0) })
        .collect();
    entries.sort_by(|a, b| a.newest.cmp(&b.newest).then_with(|| a.path.cmp(&b.path)));
    Ok((total, entries))
}

/// Deletes DerivedData entries, least recently written first: with <idle>
/// minutes > 0, each one nothing wrote to for that long, and while the folder
/// is over its cap, each one nothing wrote to in the last FLOOR_IDLE_MINUTES
/// (a one-off run between two steps holds no file open). An entry in use is
/// kept.
pub fn prune_derived_data(tool: &Tool, run: Run, idle: u64) -> Result<()> {
    let dir = tool.places.derived_data();
    if !dir.is_dir() {
        return Ok(());
    }
    let cap = DERIVED_DATA_CAP_KIB;
    let (measured, entries) = derived_data(tool)?;
    let mut total = measured.kib;
    if idle == 0 && total <= cap {
        return Ok(());
    }
    for entry in entries {
        let measured = Measure { kib: entry.kib, newest: Some(entry.newest) };
        let reason = if idle > 0 && !tool.written_within(&measured, idle) {
            format!("{}, no write in {idle}m", human(entry.kib))
        } else if total > cap && !tool.written_within(&measured, FLOOR_IDLE_MINUTES) {
            format!("{}; DerivedData is {} > {}", human(entry.kib), human(total), human(cap))
        } else {
            continue;
        };
        let deleted = if run.dry() {
            note(format!("would delete {} ({reason})", entry.path.display()));
            true
        } else if evict(&tool.places, &entry.path, Recreate::No, Removal::Background)? {
            note(format!("deleted {} ({reason})", entry.path.display()));
            true
        } else {
            note(format!("in use, kept: {} ({reason})", entry.path.display()));
            false
        };
        if deleted {
            total = total.saturating_sub(entry.kib);
        }
    }
    Ok(())
}

/// Copies the settings files <settings> that <dir> holds into a new folder
/// beside it, and returns that folder; None when <dir> holds none of them.
fn stage_settings(tool: &Tool, dir: &Path, name: &str, settings: &[&str]) -> Result<Option<PathBuf>> {
    let mut keep: Option<PathBuf> = None;
    for path in settings {
        if !dir.join(path).exists() {
            continue;
        }
        if keep.is_none() {
            keep = Some(make_temp_dir(&tool.places.caches(), &format!(".settings-{name}."))?);
        }
        let keep = keep.as_ref().expect("made above");
        let target = keep.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).context(|| format!("creating {}", parent.display()))?;
        }
        copy_preserving(&dir.join(path), &target)?;
    }
    Ok(keep)
}

/// `cp -Rp <from> <to>`: the files with their modes and times.
fn copy_preserving(from: &Path, to: &Path) -> Result<()> {
    let status = Command::new("cp")
        .arg("-Rp")
        .arg(from)
        .arg(to)
        .status()
        .context(|| format!("copying {} to {}", from.display(), to.display()))?;
    if !status.success() {
        fail!("copying {} to {} failed", from.display(), to.display());
    }
    Ok(())
}

/// Deletes each shared cache that is over its cap, whole, and recreates it
/// empty, with its settings files put back, so its link stays valid; only
/// once nothing has written to it in the last FLOOR_IDLE_MINUTES (a package
/// resolve between two fetches holds no file open) and no process holds a
/// file in it. sccache bounds its own cache.
pub fn prune_caches(tool: &Tool, run: Run) -> Result<()> {
    let caches = tool.places.caches();
    if !caches.is_dir() {
        return Ok(());
    }
    let capped: Vec<_> = SHARED_CACHES
        .iter()
        .filter(|cache| cache.name != "sccache" && caches.join(cache.name).is_dir())
        .collect();
    let dirs: Vec<PathBuf> = capped.iter().map(|cache| caches.join(cache.name)).collect();
    let measures = tool.measure_all(&dirs)?;
    for ((cache, dir), measured) in capped.into_iter().zip(dirs).zip(measures) {
        if measured.kib <= cache.cap_kib {
            continue;
        }
        let mut reason = format!("{} > {}", human(measured.kib), human(cache.cap_kib));
        if tool.written_within(&measured, FLOOR_IDLE_MINUTES) {
            note(format!(
                "over its cap but written in the last {FLOOR_IDLE_MINUTES}m, kept: {} ({reason})",
                dir.display()
            ));
        } else if run.dry() {
            note(format!("would delete {} ({reason})", dir.display()));
        } else {
            let keep = stage_settings(tool, &dir, cache.name, cache.settings)?;
            if evict(&tool.places, &dir, Recreate::Yes, Removal::Background)? {
                if let Some(keep) = &keep {
                    copy_preserving(&keep.join("."), &dir.join(""))?;
                    std::fs::remove_dir_all(keep).context(|| format!("removing {}", keep.display()))?;
                    reason += "; its settings put back";
                }
                note(format!("deleted {} ({reason}); every checkout downloads into it again", dir.display()));
            } else {
                if let Some(keep) = &keep {
                    std::fs::remove_dir_all(keep).context(|| format!("removing {}", keep.display()))?;
                }
                note(format!("in use, kept: {} ({reason})", dir.display()));
            }
        }
    }
    Ok(())
}

/// The entries one listing rule picks, deleted together, or counted on a
/// dry run. These prunes free space and change nothing a build reads, so a
/// failure, such as another session's scratch folder vanishing while it is
/// listed, is noted and the build goes on.
fn prune_listed(run: Run, what: &str, listing: std::result::Result<Vec<PathBuf>, String>) {
    let entries = match listing {
        Ok(entries) => entries,
        Err(errors) => {
            note(format!("listing {what} failed, so none were deleted: {errors}"));
            return;
        }
    };
    if entries.is_empty() {
        return;
    }
    let count = entries.len();
    if run.dry() {
        note(format!("would delete {count} {what}"));
        return;
    }
    let mut errors = Vec::new();
    for entry in &entries {
        if let Err(error) = units::remove(entry) {
            errors.push(match error {
                fail::Fail::Message(message) => message,
                fail::Fail::Exit(code) => format!("exit {code}"),
            });
        }
    }
    if errors.is_empty() {
        note(format!("deleted {count} {what}"));
    } else {
        note(format!("deleting {count} {what} failed partway: {}", errors.join(" ")));
    }
}

/// The entries directly in <folders> nothing wrote to for more than <days>
/// days, owned by <uid>, that <pick> picks.
fn old_entries(
    folders: &[PathBuf],
    days: i64,
    uid: u32,
    now: i64,
    pick: impl Fn(&str, &std::fs::Metadata) -> bool,
) -> std::result::Result<Vec<PathBuf>, String> {
    use std::os::unix::fs::MetadataExt;
    let mut found = Vec::new();
    for folder in folders {
        let entries = std::fs::read_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("{}: {error}", folder.display()))?;
            let meta = match std::fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("{}: {error}", entry.path().display())),
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if now - meta.mtime() > days * 86400 && meta.uid() == uid && pick(&name, &meta) {
                found.push(entry.path());
            }
        }
    }
    Ok(found)
}

/// Claude Code sessions' scratch folders (/private/tmp/claude-<uid>/<project>/
/// <session>/scratchpad): an entry nothing has written for three days is a
/// finished probe, build or log. Anything worth keeping belongs in the repo or
/// its plans/.
pub fn prune_scratchpads(tool: &Tool, run: Run) -> Result<()> {
    let base = tool.places.scratch_base();
    let mut folders = Vec::new();
    for project in visible_names(&base) {
        let project = base.join(project);
        for session in visible_names(&project) {
            let scratchpad = project.join(session).join("scratchpad");
            if scratchpad.is_dir() {
                folders.push(scratchpad);
            }
        }
    }
    if folders.is_empty() {
        return Ok(());
    }
    let listing = old_entries(&folders, 3, tool.places.uid, tool.now, |_, _| true);
    prune_listed(run, &format!("scratch entries older than three days under {}", base.display()), listing);
    Ok(())
}

/// The names a shell's `*` matches in <dir>; none when it can't be listed.
fn visible_names(dir: &Path) -> Vec<std::ffi::OsString> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name())
        .filter(|name| name.as_encoded_bytes().first() != Some(&b'.'))
        .collect()
}

/// Tests' temporary folders (tempfile's `.tmp*` under $TMPDIR) that a test
/// left behind: nothing reads one a day after it was last written. Newer ones
/// may belong to a test still running.
pub fn prune_test_temps(tool: &Tool, run: Run) -> Result<()> {
    let Some(tmpdir) = &tool.places.tmpdir else { return Ok(()) };
    if !tmpdir.is_dir() {
        return Ok(());
    }
    let listing = old_entries(std::slice::from_ref(tmpdir), 1, tool.places.uid, tool.now, |name, meta| {
        name.starts_with(".tmp") && meta.is_dir()
    });
    prune_listed(run, &format!("test temp folders older than a day in {}", tmpdir.display()), listing);
    Ok(())
}

/// Build output in the temporary folders: each one nothing wrote to in the
/// last FLOOR_IDLE_MINUTES (a build between two of its steps holds no file
/// open) and no process holds a file in is deleted.
pub fn prune_temp_builds(tool: &Tool, run: Run) -> Result<()> {
    let folders = temp_build_folders(&tool.places)?;
    let measures = tool.measure_all(&folders)?;
    for (folder, measured) in folders.into_iter().zip(measures) {
        if tool.written_within(&measured, FLOOR_IDLE_MINUTES) {
            continue;
        }
        let what = format!(
            "{} of build output in a temporary folder, no write in {FLOOR_IDLE_MINUTES}m",
            human(measured.kib)
        );
        if run.dry() {
            note(format!("would delete {} ({what})", folder.display()));
        } else if evict(&tool.places, &folder, Recreate::No, Removal::Background)? {
            note(format!("deleted {} ({what})", folder.display()));
        } else {
            note(format!("in use, kept: {} (build output in a temporary folder)", folder.display()));
        }
    }
    Ok(())
}

/// Cargo's git checkouts: each one no Cargo.lock of a live checkout this
/// tool knows pins goes. The git databases stay, and cargo checks a commit
/// out of them again in seconds when a branch needs it.
pub fn prune_cargo_git(tool: &Tool, roots: &[Root], run: Run) -> Result<()> {
    let tops: Vec<PathBuf> = roots.iter().filter_map(|root| root.live.as_ref()).map(|c| c.top.clone()).collect();
    let listed = crate::checkout::parallel_map(&tops, |top| {
        git_text(git(top).args(["ls-files", "--", "Cargo.lock", "*/Cargo.lock"]), "list Cargo.lock files")
            .map(|text| text.lines().map(|line| top.join(line)).collect::<Vec<_>>())
    });
    let mut lockfiles = Vec::new();
    for found in listed {
        lockfiles.extend(found?);
    }
    // With no lockfile to read, every checkout would read as unpinned.
    if lockfiles.is_empty() {
        return Ok(());
    }
    units::git_checkouts::prune_git_checkouts(&tool.places.cargo_home, run.dry(), &lockfiles)?;
    Ok(())
}

/// `builds prune [--dry-run] [--idle <minutes> [--include-warm]]`.
pub fn cmd_prune(tool: &Tool, args: &[String]) -> Result<()> {
    let mut run = Run::Real;
    let mut idle: u64 = 0;
    let mut warm = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--dry-run" => run = Run::Dry,
            "--include-warm" => warm = true,
            "--idle" => {
                let minutes = rest.next().map(String::as_str).unwrap_or("");
                if minutes.is_empty() || !minutes.bytes().all(|b| b.is_ascii_digit()) {
                    fail!("prune: --idle takes minutes");
                }
                idle = minutes.parse().context(|| "prune: --idle takes minutes".to_string())?;
            }
            other => fail!("prune: unknown option {other}"),
        }
    }
    if warm && idle == 0 {
        fail!("prune: --include-warm needs --idle");
    }
    tool.places.require_volume()?;
    let roots = tool.roots()?;
    for root in &roots {
        let Some(checkout) = &root.live else {
            let (path, top) = (root.path.display(), root.top.display());
            if run.dry() {
                note(format!("would delete {path} (checkout {top} is gone)"));
            } else if evict(&tool.places, &root.path, Recreate::No, Removal::Background)? {
                note(format!("deleted {path} (checkout {top} is gone)"));
            } else {
                note(format!("checkout {top} is gone but {path} is in use, kept"));
            }
            continue;
        };
        let idle_here = if !warm && root.keeps_warm(&tool.places)? { 0 } else { idle };
        prune_root(tool, &root.path, checkout, run, idle_here)?;
    }
    let derived_idle = if idle > 0 && idle < DERIVED_DATA_IDLE_MINUTES { idle } else { DERIVED_DATA_IDLE_MINUTES };
    prune_derived_data(tool, run, derived_idle)?;
    prune_caches(tool, run)?;
    prune_test_temps(tool, run)?;
    prune_temp_builds(tool, run)?;
    prune_scratchpads(tool, run)?;
    prune_cargo_git(tool, &roots, run)?;
    if !run.dry() {
        empty_trash(tool)?;
    }
    Ok(())
}

/// Starts removing everything in the trash in the background.
pub fn empty_trash(tool: &Tool) -> Result<()> {
    let trash = tool.places.trash();
    if trash.is_dir() {
        background_rm(&trash)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "prune_tests.rs"]
mod tests;

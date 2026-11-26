//! Bounding what builds leave behind: each checkout's unused build copies and
//! caps, DerivedData, the shared caches, the shots runs (shots_cache.rs),
//! build output in the temporary folders, tests' temporary folders, Claude
//! Code scratch folders, cargo's git checkouts, and the tool versions a repo's scripts install that no
//! checkout runs. `setup` runs these for its checkout, `prune` for every one.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::checkout::{Checkout, Root};
use crate::evict::{Recreate, Removal, evict};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::free_worktrees::retire_free;
use crate::manifest::{SHARED_CACHES, SharedCache, build_dirs, has_manifest};
use crate::places::{DERIVED_DATA_CAP_KIB, DERIVED_DATA_IDLE_MINUTES, FLOOR_IDLE_MINUTES, Places};
use crate::temp_builds::temp_build_folders;
use crate::text::{human, note};
use crate::tool::Tool;
use crate::trees::prune_trees;
use crate::units;
use crate::usage::Measure;
use boards::{git, git_text};

/// The command that requests cleanup. Setup warns about refused moves and
/// still checks the hard free-space floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pruner {
    Setup,
    Prune,
}

impl Pruner {
    pub(crate) fn after_move<T>(self, result: Result<T>) -> Result<Option<T>> {
        match (self, result) {
            (Self::Setup, Err(error @ Fail::Move { .. })) => {
                note(format!("warning: {}; builds kept the entry", error.text()));
                Ok(None)
            }
            (_, result) => result.map(Some),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    Real,
    Dry,
}

impl Run {
    pub fn dry(self) -> bool {
        self == Run::Dry
    }

    /// The verb for what a deletion did in this run: "deleted", or on a dry
    /// run "would delete".
    pub fn deleted(self) -> &'static str {
        match self {
            Run::Real => "deleted",
            Run::Dry => "would delete",
        }
    }
}

/// Deletes the unused build copies in the folders of <checkout>'s manifest in
/// <root>, then applies the manifest's caps to them; with <idle> minutes > 0,
/// also deletes folders nothing wrote to for that long, and with <warm_idle>
/// above 0 the same for the folders that stay warm in <root> (main's, an app
/// checkout's own lane's).
pub fn prune_root(
    tool: &Tool,
    root: &Path,
    checkout: &Checkout,
    run: Run,
    idle: u64,
    warm_idle: u64,
    pruner: Pruner,
) -> Result<()> {
    let manifest = build_dirs(&checkout.top)?;
    let folders: Vec<PathBuf> = manifest
        .iter()
        .map(|entry| root.join(&entry.path))
        .filter(|folder| folder.is_dir())
        .collect();
    if folders.is_empty() {
        return Ok(());
    }
    let mut freed = units::prune::prune(&cargo_ages(tool), run, &folders)?;
    freed.extend(prune_trees(tool, root, checkout, &folders, run, pruner)?);
    let claimed = checkout.claim_file().is_file();
    let lane = checkout.app_lane(&tool.places)?;
    // Measured together, after the pruning: deleting one folder changes no
    // other's size.
    let measures = tool.measure_all(&folders)?;
    for entry in &manifest {
        let folder = root.join(&entry.path);
        let Some(at) = folders.iter().position(|f| *f == folder) else {
            continue;
        };
        let measured = measures[at];
        let mut kib = measured.kib;
        if run.dry() {
            // Nothing was deleted; measure the folder as pruning would leave
            // it.
            let pruned: u64 = freed
                .iter()
                .filter(|(f, _)| *f == folder)
                .map(|(_, kib)| *kib)
                .sum();
            kib = kib.saturating_sub(pruned);
        }
        let app = entry.is_app_in(lane);
        let cap_kib = entry.cap_kib(app);
        let idle = if checkout.is_main() || app {
            warm_idle
        } else {
            idle
        };
        let reason = if kib > cap_kib {
            format!("{} > {}", human(kib), human(cap_kib))
        } else if idle > 0 && kib > 1024 && !tool.written_within(&measured, idle) {
            format!("{}, no write in {idle}m", human(kib))
        } else {
            continue;
        };
        // A claimed worktree's agent is mid-task; emptying its folder costs it
        // a cold rebuild of every dependency, as the floor's exemption says.
        if claimed {
            note(format!("claimed, kept: {} ({reason})", folder.display()));
            continue;
        }
        let deleted = delete_folder(
            tool,
            &folder,
            run,
            &reason,
            Recreate::Yes,
            Removal::Background,
            "; the next build refills it",
        );
        pruner.after_move(deleted)?;
    }
    Ok(())
}

/// The rules for a checkout's build copies. In every checkout, `units::prune`
/// deletes an incremental folder once nothing has written to it for the idle
/// hours: main's builds compile incrementally, and in the other checkouts a
/// repo's lint wrapper may keep an incremental folder for the crates it lints
/// (bae's scripts/clippy-rustc-wrapper.sh) while every other crate builds
/// without incremental compilation. The build folder's cap bounds what is
/// left.
fn cargo_ages(tool: &Tool) -> units::prune::Ages {
    units::prune::Ages::new(tool.rules.unused_days, tool.rules.incremental_hours)
}

/// `builds units after-build <checkout>`, which `builds cargo` starts as it
/// exits: deletes the build copies in <checkout>'s folders that the build
/// replaced, and the rest `units::prune` deletes, once nothing holds the
/// checkout's guard or a profile folder's lock.
pub fn prune_after_build(tool: &Tool, checkout: &Checkout) -> Result<()> {
    if !has_manifest(&checkout.top) {
        return Ok(());
    }
    let root = checkout.build_root(&tool.places);
    let folders: Vec<PathBuf> = build_dirs(&checkout.top)?
        .iter()
        .map(|entry| root.join(&entry.path))
        .filter(|folder| folder.is_dir())
        .collect();
    units::prune::prune(&cargo_ages(tool), Run::Real, &folders)?;
    Ok(())
}

/// Deletes <folder> for <reason>, removing it as <removal> says, or says it
/// would, or that it is in use. Returns whether it went, or on a dry run
/// would go.
pub fn delete_folder(
    tool: &Tool,
    folder: &Path,
    run: Run,
    reason: &str,
    recreate: Recreate,
    removal: Removal,
    after: &str,
) -> Result<bool> {
    if run.dry() {
        note(format!("would delete {} ({reason})", folder.display()));
        Ok(true)
    } else if evict(&tool.places, folder, recreate, removal)? {
        note(format!("deleted {} ({reason}){after}", folder.display()));
        Ok(true)
    } else {
        note(format!("in use, kept: {} ({reason})", folder.display()));
        Ok(false)
    }
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
        .map(|(path, measured)| Entry {
            path,
            kib: measured.kib,
            newest: measured.newest.unwrap_or(0),
        })
        .collect();
    entries.sort_by(|a, b| a.newest.cmp(&b.newest).then_with(|| a.path.cmp(&b.path)));
    Ok((total, entries))
}

/// Deletes DerivedData entries, least recently written first: with <idle>
/// minutes > 0, each one nothing wrote to for that long, and while the folder
/// is over its cap, each one nothing wrote to in the last FLOOR_IDLE_MINUTES
/// (a one-off run between two steps holds no file open). An entry in use is
/// kept.
pub fn prune_derived_data(tool: &Tool, run: Run, idle: u64, pruner: Pruner) -> Result<()> {
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
        let measured = Measure {
            kib: entry.kib,
            newest: Some(entry.newest),
        };
        let reason = if idle > 0 && !tool.written_within(&measured, idle) {
            format!("{}, no write in {idle}m", human(entry.kib))
        } else if total > cap && !tool.written_within(&measured, FLOOR_IDLE_MINUTES) {
            format!(
                "{}; DerivedData is {} > {}",
                human(entry.kib),
                human(total),
                human(cap)
            )
        } else {
            continue;
        };
        let deleted = delete_folder(
            tool,
            &entry.path,
            run,
            &reason,
            Recreate::No,
            Removal::Background,
            "",
        );
        if pruner.after_move(deleted)? == Some(true) {
            total = total.saturating_sub(entry.kib);
        }
    }
    Ok(())
}

/// Copies the settings the live cache holds into an empty replacement.
fn copy_settings(
    dir: &Path,
    replacement: &Path,
    settings: &[&str],
    copy: &impl Fn(&Path, &Path) -> Result<()>,
) -> Result<()> {
    for path in settings {
        let from = dir.join(path);
        if crate::whole_file::lstat(&from)?.is_none() {
            continue;
        }
        let target = replacement.join(path);
        let parent = target
            .parent()
            .expect("settings paths lie inside the cache");
        std::fs::create_dir_all(parent).context(|| format!("creating {}", parent.display()))?;
        copy(&from, &target)?;
    }
    Ok(())
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

/// Replaces each shared cache that is over its cap with an otherwise empty
/// folder holding its settings, so the live path and its links stay valid; only
/// once nothing has written to it in the last FLOOR_IDLE_MINUTES (a package
/// resolve between two fetches holds no file open) and no process holds a
/// file in it. sccache bounds its own cache.
fn prune_caches(
    tool: &Tool,
    run: Run,
    pruner: Pruner,
    capped: &[SharedCache],
    copy: impl Fn(&Path, &Path) -> Result<()>,
) -> Result<()> {
    let caches = tool.places.caches();
    if !caches.is_dir() {
        return Ok(());
    }
    // A second prune must measure and copy the live cache after the first
    // one finishes replacing it. Other builds do not take this lock.
    let _pruning = if run.dry() {
        None
    } else {
        let lock = tool.places.root.join(".shared-cache-pruning");
        let Some(pruning) = crate::evict::try_lock_creating(&lock)? else {
            note("Another prune is clearing shared caches.");
            return Ok(());
        };
        Some(pruning)
    };
    let capped: Vec<_> = capped
        .iter()
        .filter(|cache| cache.name != "sccache" && caches.join(cache.name).is_dir())
        .collect();
    let dirs: Vec<PathBuf> = capped.iter().map(|cache| caches.join(cache.name)).collect();
    let measures = tool.measure_all(&dirs)?;
    for ((cache, dir), measured) in capped.into_iter().zip(dirs).zip(measures) {
        if measured.kib <= cache.cap_kib {
            continue;
        }
        let reason = format!("{} > {}", human(measured.kib), human(cache.cap_kib));
        if tool.written_within(&measured, FLOOR_IDLE_MINUTES) {
            note(format!(
                "over its cap but written in the last {FLOOR_IDLE_MINUTES}m, kept: {} ({reason})",
                dir.display()
            ));
        } else if run.dry() {
            note(format!("would delete {} ({reason})", dir.display()));
        } else {
            let moved = crate::evict::replace(&tool.places, &dir, &|live, replacement| {
                copy_settings(live, replacement, cache.settings, &copy)
            });
            match pruner.after_move(moved)? {
                Some(true) => note(format!(
                    "deleted {} ({reason}); every checkout downloads into it again",
                    dir.display()
                )),
                Some(false) => note(format!("in use, kept: {} ({reason})", dir.display())),
                None => {}
            }
        }
    }
    Ok(())
}

/// Prunes caches and temporary output both setup and explicit prune own.
pub fn prune_shared(tool: &Tool, run: Run, pruner: Pruner) -> Result<()> {
    prune_caches(tool, run, pruner, SHARED_CACHES, copy_preserving)?;
    pruner.after_move(crate::shots_cache::prune(tool, run))?;
    pruner.after_move(crate::review::texts::prune(tool, run))?;
    pruner.after_move(crate::agent_prune::prune(tool, run))?;
    prune_test_temps(tool, run)?;
    prune_temp_builds(tool, run, pruner)?;
    prune_scratchpads(tool, run)
}

/// Prune build tools and check free space after the other cleanup steps.
pub fn prune_tools_and_floor(tool: &Tool, roots: &[Root], run: Run, pruner: Pruner) -> Result<()> {
    prune_cargo_git(tool, roots, run)?;
    prune_installed(tool, roots, run)?;
    crate::built_tools::prune_tools(&tool.places, roots, run)?;
    crate::floor::keep_floor(tool, roots, pruner, run)
}

/// The entries one listing rule picks, deleted together, or counted on a
/// dry run. These prunes free space and change nothing a build reads, so a
/// failure, such as another session's scratch folder vanishing while it is
/// listed, is noted and the build goes on.
pub fn prune_listed(run: Run, what: &str, listing: std::result::Result<Vec<PathBuf>, String>) {
    let entries = match listing {
        Ok(entries) => entries,
        Err(errors) => {
            note(format!(
                "listing {what} failed, so none were deleted: {errors}"
            ));
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
            errors.push(error.text());
        }
    }
    if errors.is_empty() {
        note(format!("deleted {count} {what}"));
    } else {
        note(format!(
            "deleting {count} {what} failed partway: {}",
            errors.join(" ")
        ));
    }
}

/// The entries directly in <folders> nothing wrote to for more than <days>
/// days, owned by <uid>, that <pick> picks.
pub fn old_entries(
    folders: &[PathBuf],
    days: i64,
    uid: u32,
    now: i64,
    pick: impl Fn(&str, &std::fs::Metadata) -> bool,
) -> std::result::Result<Vec<PathBuf>, String> {
    use std::os::unix::fs::MetadataExt;
    let mut found = Vec::new();
    for folder in folders {
        let entries =
            std::fs::read_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
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
/// <session>/scratchpad) of sessions no longer running: an entry nothing
/// anywhere inside has been written in for three days is a finished probe,
/// build or log. A running session's scratchpad is never touched: its notes
/// may be copies it refreshes in place, which leave the entry's own time
/// alone. Anything worth keeping belongs in the repo or its plans/.
pub fn prune_scratchpads(tool: &Tool, run: Run) -> Result<()> {
    let base = tool.places.scratch_base();
    let live = match live_sessions(&tool.places.claude_sessions()) {
        Ok(live) => live,
        Err(error) => {
            note(format!(
                "reading the running Claude Code sessions failed, so no scratch entry was deleted: {error}"
            ));
            return Ok(());
        }
    };
    let mut folders = Vec::new();
    for project in visible_names(&base) {
        let project = base.join(project);
        for session in visible_names(&project) {
            if live.contains(&session) {
                continue;
            }
            let scratchpad = project.join(session).join("scratchpad");
            if scratchpad.is_dir() {
                folders.push(scratchpad);
            }
        }
    }
    if folders.is_empty() {
        return Ok(());
    }
    // An entry written in itself lately is not idle; the rest are measured,
    // since a write deeper inside leaves the entry's own time alone.
    let listing = match old_entries(&folders, 3, tool.places.uid, tool.now, |_, _| true) {
        Ok(candidates) => {
            let measures = tool.measure_all(&candidates)?;
            Ok(candidates
                .into_iter()
                .zip(measures)
                .filter(|(_, measured)| !tool.written_within(measured, 3 * 24 * 60))
                .map(|(path, _)| path)
                .collect())
        }
        Err(error) => Err(error),
    };
    prune_listed(
        run,
        &format!(
            "scratch entries of ended sessions with no write inside for three days under {}",
            base.display()
        ),
        listing,
    );
    Ok(())
}

/// Deletes each session's folder of agent registrations (ack/agent.rs) whose
/// session Claude Code's record no longer lists as running. When builds cannot
/// read the record, it deletes nothing and says so.
pub fn prune_agents(tool: &Tool, run: Run) -> Result<()> {
    let live = match live_sessions(&tool.places.claude_sessions()) {
        Ok(live) => live,
        Err(error) => {
            note(format!(
                "reading the running Claude Code sessions failed, so no agent registration was deleted: {error}"
            ));
            return Ok(());
        }
    };
    prune_listed(
        run,
        &format!(
            "agent registration folders of ended sessions under {}",
            crate::ack::agent::dir(&tool.places)?.display()
        ),
        crate::ack::agent::ended_sessions(&tool.places, &live, tool.now),
    );
    Ok(())
}

/// The ids of the Claude Code sessions still running.
pub fn live_sessions(
    sessions: &Path,
) -> std::result::Result<std::collections::HashSet<std::ffi::OsString>, String> {
    Ok(crate::claude_sessions::running_now(sessions)?
        .into_iter()
        .map(|running| running.session_id.into())
        .collect())
}

/// The names a shell's `*` matches in <dir>; none when it can't be listed.
fn visible_names(dir: &Path) -> Vec<std::ffi::OsString> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name())
        .filter(|name| name.as_encoded_bytes().first() != Some(&b'.'))
        .collect()
}

/// Temporary folders a test or a Swift tool left behind under $TMPDIR
/// (tempfile's `.tmp*`, SwiftPM's and swift-driver's `TemporaryDirectory.*`,
/// 84,000 of them in a few weeks): nothing reads one a day after it was last
/// written. Newer ones may belong to a run still going.
pub fn prune_test_temps(tool: &Tool, run: Run) -> Result<()> {
    let Some(tmpdir) = &tool.places.tmpdir else {
        return Ok(());
    };
    if !tmpdir.is_dir() {
        return Ok(());
    }
    let listing = old_entries(
        std::slice::from_ref(tmpdir),
        1,
        tool.places.uid,
        tool.now,
        |name, meta| {
            (name.starts_with(".tmp") || name.starts_with("TemporaryDirectory.")) && meta.is_dir()
        },
    );
    prune_listed(
        run,
        &format!(
            "temporary folders of tests and Swift tools older than a day in {}",
            tmpdir.display()
        ),
        listing,
    );
    Ok(())
}

/// Build output in the temporary folders: each one nothing wrote to in the
/// last FLOOR_IDLE_MINUTES (a build between two of its steps holds no file
/// open) and no process holds a file in is deleted.
fn prune_temp_builds(tool: &Tool, run: Run, pruner: Pruner) -> Result<()> {
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
        let deleted = delete_folder(
            tool,
            &folder,
            run,
            &what,
            Recreate::No,
            Removal::Background,
            "",
        );
        pruner.after_move(deleted)?;
    }
    Ok(())
}

/// Cargo's git checkouts: each one no Cargo.lock of a live checkout this
/// tool knows pins goes. The git databases stay, and cargo checks a commit
/// out of them again in seconds when a branch needs it.
fn prune_cargo_git(tool: &Tool, roots: &[Root], run: Run) -> Result<()> {
    let live: Vec<&Root> = roots.iter().filter(|root| root.live.is_some()).collect();
    let listed = crate::checkout::parallel_map(&live, |root| lockfiles_of(&tool.places, root));
    let mut lockfiles = Vec::new();
    for found in listed {
        lockfiles.extend(found?);
    }
    // With no lockfile to read, every checkout would read as unpinned.
    if lockfiles.is_empty() {
        return Ok(());
    }
    units::git_checkouts::prune_git_checkouts(&tool.places.cargo_home, run, &lockfiles)?;
    Ok(())
}

/// The Cargo.lock files git tracks in the checkout of <root>, which was live
/// when the caller listed the roots. Another builds process may remove the
/// checkout meanwhile (a retired side worktree, a host's idle app checkout or
/// job worktree): git deletes the worktree's files, its `.git` among them,
/// before the remover deletes the build root. A checkout that is no longer
/// live pins nothing; git failing in one that still is fails.
fn lockfiles_of(places: &Places, root: &Root) -> Result<Vec<PathBuf>> {
    let top = &root.top;
    match git_text(
        git(top).args(["ls-files", "--", "Cargo.lock", "*/Cargo.lock"]),
        "list Cargo.lock files",
    ) {
        Ok(text) => Ok(text.lines().map(|line| top.join(line)).collect()),
        Err(error) => match Root::read(places, &root.path)? {
            Some(Root { live: Some(_), .. }) => Err(error.into()),
            _ => {
                note(format!(
                    "pruning skipped {top}: git could not list its Cargo.lock files ({}), and {top} is no longer a live checkout, so it pins nothing",
                    error.text(),
                    top = top.display()
                ));
                Ok(Vec::new())
            }
        },
    }
}

/// The tools a repo's script installs once per version into the cache home
/// (`.build-dirs` `installed` lines). Each version is an entry in the
/// tool's folder named after it, and pruning deletes it once no live
/// checkout's file names it. Pruning reads the file in every live checkout,
/// whether or not the checkout's own manifest has the line, so a checkout on
/// a branch from before the line keeps its version. Pruning leaves an entry
/// whose name starts with a dot, since a script is still installing it. When
/// pruning can't read a manifest, the folder or a file, it deletes nothing
/// that the unread text could name, and notes why.
fn prune_installed(tool: &Tool, roots: &[Root], run: Run) -> Result<()> {
    let tops: Vec<&Path> = roots
        .iter()
        .filter_map(|root| root.live.as_ref())
        .map(|checkout| checkout.top.as_path())
        .collect();
    let mut files_of: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for top in &tops {
        let declared = match crate::manifest::installed(top) {
            Ok(declared) => declared,
            Err(error) => {
                note(format!(
                    "pruning could not read {}'s {}, so it deleted no installed tool: {}",
                    top.display(),
                    crate::manifest::MANIFEST,
                    error.text()
                ));
                return Ok(());
            }
        };
        for line in declared {
            let files = files_of.entry(line.folder).or_default();
            if !files.contains(&line.file) {
                files.push(line.file);
            }
        }
    }
    for (folder, files) in files_of {
        let dir = tool.places.cache_home.join(&folder);
        let what = format!(
            "installed versions in {} that no checkout's {} names",
            dir.display(),
            files.join(" or ")
        );
        // Pruning lists the versions before it reads the files. A script
        // writes a version into its file before it installs that version, so
        // when pruning reads the files, the file of each checkout that runs a
        // listed version already names it.
        let versions = match version_names(&dir) {
            Ok(Some(versions)) if !versions.is_empty() => versions,
            Ok(_) => continue,
            Err(error) => {
                prune_listed(run, &what, Err(error));
                continue;
            }
        };
        let listing = match naming_texts(&tops, &files) {
            // With no text to search, no file names any version, and pruning
            // would delete every version.
            Ok(texts) if texts.is_empty() => {
                note(format!(
                    "pruning deleted nothing in {}: no live checkout has {}",
                    dir.display(),
                    files.join(" or ")
                ));
                continue;
            }
            Ok(texts) => Ok(versions
                .into_iter()
                .filter(|version| {
                    let version = version.to_string_lossy();
                    !texts.iter().any(|text| text.contains(&*version))
                })
                .map(|version| dir.join(version))
                .collect()),
            Err(error) => Err(error),
        };
        prune_listed(run, &what, listing);
    }
    Ok(())
}

/// The names in <dir> that don't start with a dot, which are the versions
/// installed there; None when <dir> doesn't exist, since then no script has
/// installed a version yet.
fn version_names(dir: &Path) -> std::result::Result<Option<Vec<std::ffi::OsString>>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let name = entry
            .map_err(|error| format!("{}: {error}", dir.display()))?
            .file_name();
        if name.as_encoded_bytes().first() != Some(&b'.') {
            names.push(name);
        }
    }
    Ok(Some(names))
}

/// The text of each of <files> in each checkout of <tops> that has it.
fn naming_texts(tops: &[&Path], files: &[String]) -> std::result::Result<Vec<String>, String> {
    let mut texts = Vec::new();
    for top in tops {
        for file in files {
            let path = top.join(file);
            match std::fs::read_to_string(&path) {
                Ok(text) => texts.push(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
    }
    Ok(texts)
}

/// `builds prune [--dry-run] [--idle <minutes> [--include-warm]]`. Also
/// retires the free side worktrees, removes the
/// worktrees on the hosts that no checkout's jobs use, and runs each host's
/// own prune with the same options (`prune_hosts`). A prune that
/// deletes holds the prune lock while it runs and records when it finished
/// (prune_schedule.rs).
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
                idle = minutes
                    .parse()
                    .context(|| "prune: --idle takes minutes".to_string())?;
            }
            other => fail!("prune: unknown option {other}"),
        }
    }
    if warm && idle == 0 {
        fail!("prune: --include-warm needs --idle");
    }
    tool.places.require_volume()?;
    let _running = if run.dry() {
        None
    } else {
        Some(crate::prune_schedule::hold(&tool.places)?)
    };
    // Before the roots are pruned: a worktree about to go has nothing to prune.
    retire_free(tool, &tool.roots()?, run)?;
    // Remove the host worktrees after retire_free runs, because the jobs of
    // a worktree it retires no longer use that worktree's copies on the
    // hosts.
    let ssh = crate::host::Ssh::from_env();
    crate::remote_worktrees::prune(&tool.places, &ssh, run)?;
    crate::lane_size::shrink_all(&tool.places, &ssh, run, crate::app_timing::now())?;
    prune_hosts(&tool.places, &ssh, args)?;
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
        prune_root(
            tool,
            &root.path,
            checkout,
            run,
            idle,
            if warm { idle } else { 0 },
            Pruner::Prune,
        )?;
    }
    let derived_idle = if idle > 0 && idle < DERIVED_DATA_IDLE_MINUTES {
        idle
    } else {
        DERIVED_DATA_IDLE_MINUTES
    };
    prune_derived_data(tool, run, derived_idle, Pruner::Prune)?;
    prune_shared(tool, run, Pruner::Prune)?;
    prune_agents(tool, run)?;
    prune_tools_and_floor(tool, &roots, run, Pruner::Prune)?;
    if !run.dry() {
        empty_trash(tool)?;
        crate::prune_schedule::record_finished(&tool.places, crate::app_timing::now())?;
    }
    Ok(())
}

/// Runs each recorded host's own `builds prune` with <options>, this
/// prune's, after this prune removed the worktrees and app checkouts there
/// that no checkout here uses: the host deletes the build roots of the
/// checkouts gone there, applies the caps and the rest to its own folders,
/// and keeps its app checkouts warm (`Checkout::app_lane`). What the host's
/// prune says comes out here behind the host's name. When a host does not
/// answer or its prune fails, prune notes it and moves on to the next host.
fn prune_hosts(
    places: &crate::places::Places,
    ssh: &crate::host::Ssh,
    options: &[String],
) -> Result<()> {
    let words: Vec<String> = std::iter::once("prune".to_string())
        .chain(options.iter().map(|option| crate::host::sh_quote(option)))
        .collect();
    for host in crate::host::read_remote(places)? {
        match crate::host_setup::ask_builds_there(places, ssh, &host, &words.join(" "), "pruning")?
        {
            Ok(said) => {
                for line in said.lines().filter(|line| !line.is_empty()) {
                    let line = line.strip_prefix("builds: ").unwrap_or(line);
                    note(format!("host {}: {line}", host.name));
                }
            }
            Err(said) => note(said),
        }
    }
    Ok(())
}

/// Starts removing everything in the trash in the background.
pub fn empty_trash(tool: &Tool) -> Result<()> {
    crate::trash::empty_background(&tool.places)
}

#[cfg(test)]
#[path = "prune_tests.rs"]
mod tests;

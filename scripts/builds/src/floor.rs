//! The free-space floor.
//!
//! Caps bound each folder, not the volume: a checkout with every platform
//! built is 45-52 GB, and the build volume shares its APFS pool with the main
//! disk. So `setup` keeps a free-space floor, and `prune` keeps the volume
//! above it ahead of time. Each deletes DerivedData's idle entries, then the
//! incremental folders nothing wrote to for FLOOR_INCREMENTAL_IDLE_MINUTES in
//! every checkout, then whole idle folders of the side worktrees no agent
//! claims and of a host's job worktrees no job claims, least recently written
//! first, never main's or an app checkout's own lane's, whose builds stay warm
//! on purpose. It goes on until the volume has the floor plus its margin
//! free, so the next build's writes don't take the volume under the floor
//! again at once. If it can't reach the floor it warns, naming the biggest
//! folders, and the build goes on: whoever runs the agents frees space or
//! runs fewer of them. Only under the hard floor does setup fail, rather than
//! let the build run out of space halfway. It never deletes a shared cache.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::checkout::{Checkout, Root};
use crate::evict::{Recreate, Removal};
use crate::fail::{Fail, Result};
use crate::manifest::{SHARED_CACHES, build_dirs};
use crate::output::note;
use crate::places::{FLOOR_IDLE_MINUTES, FLOOR_INCREMENTAL_IDLE_MINUTES};
use crate::prune::{Pruner, Run, delete_folder, derived_data};
use crate::text::{counted, human, human_bytes, local_minute};
use crate::tool::Tool;
use crate::units::OpenFiles;
use crate::units::idle::{Outcome, delete_idle, idle_increments};
use crate::usage::Measure;

/// A build folder the floor may delete, with what it takes.
struct Candidate {
    path: PathBuf,
    measured: Measure,
}

/// Is <checkout> one that no agent or job works in: a side worktree no agent
/// claims, or a job worktree on a host that no job claims? A claimed
/// worktree has an agent or a job at work in it; an agent may go quiet for an
/// hour between builds, and deleting its folders costs it a cold rebuild of
/// every dependency.
fn unclaimed(checkout: &Checkout) -> bool {
    (checkout.is_side() && !checkout.claim_file().is_file())
        || (checkout.is_job() && !checkout.job_claim_folder().exists())
}

/// The build folders the floor may delete, least recently written first:
/// every unclaimed side or job worktree's (`unclaimed`), and every app
/// checkout's but those its lane builds into.
fn floor_candidates(tool: &Tool, roots: &[Root]) -> Result<Vec<Candidate>> {
    let mut paths = Vec::new();
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        let lane = root.app_lane(&tool.places)?;
        if lane.is_none() && !unclaimed(checkout) {
            continue;
        }
        for entry in build_dirs(&checkout.top)? {
            // An app checkout's folders of the other lane are not warm: nothing
            // builds into them there.
            if entry.is_app_in(lane) {
                continue;
            }
            let folder = root.path.join(&entry.path);
            if folder.is_dir() {
                paths.push(folder);
            }
        }
    }
    let measured = tool.measure_all(&paths)?;
    let mut candidates: Vec<Candidate> = paths
        .into_iter()
        .zip(measured)
        .map(|(path, measured)| Candidate { path, measured })
        .collect();
    candidates.sort_by(|a, b| {
        a.measured
            .newest
            .unwrap_or(0)
            .cmp(&b.measured.newest.unwrap_or(0))
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(candidates)
}

/// The volume's free space as the floor's deletions go: read again after
/// each deletion, or on a dry run, the first reading plus what each deletion
/// would free.
struct Free<'a> {
    tool: &'a Tool,
    run: Run,
    pruner: Pruner,
    kib: u64,
}

impl Free<'_> {
    /// Counts a deletion that freed, or would free, <kib>.
    fn freed(&mut self, kib: u64) -> Result<()> {
        self.kib = if self.run.dry() {
            self.kib + kib
        } else {
            self.tool.free_kib()?
        };
        Ok(())
    }

    /// Deletes <path>, which takes <kib>, for <why>, waiting for the removal
    /// so the next reading counts it, or on a dry run says it would; a
    /// folder in use stays.
    fn delete(&mut self, path: &Path, kib: u64, recreate: Recreate, why: &str) -> Result<()> {
        let deleted = delete_folder(self.tool, path, self.run, why, recreate, Removal::Now, "");
        if self.pruner.after_move(deleted)? == Some(true) {
            self.freed(kib)?;
        }
        Ok(())
    }
}

/// What became of one profile folder's idle incremental folders.
#[derive(Default)]
struct Tally {
    deleted: usize,
    bytes: u64,
    locked: usize,
    held: usize,
}

/// Deletes the incremental folders nothing wrote anywhere inside for
/// FLOOR_INCREMENTAL_IDLE_MINUTES, in every live checkout's build folders,
/// claimed ones, main's and the app checkout's too, least recently written
/// first, until the volume has <target> free; says what went from each
/// profile folder and what stayed.
fn delete_idle_increments(tool: &Tool, roots: &[Root], free: &mut Free, target: u64) -> Result<()> {
    let mut folders = Vec::new();
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        for entry in build_dirs(&checkout.top)? {
            let folder = root.path.join(&entry.path);
            if folder.is_dir() {
                // lsof names files by their real path.
                folders.push(std::fs::canonicalize(&folder).unwrap_or(folder));
            }
        }
    }
    let before = (tool.now - (FLOOR_INCREMENTAL_IDLE_MINUTES * 60) as i64) as f64;
    let increments = idle_increments(&folders, before)?;
    let mut open_files = OpenFiles::new(&tool.system, &folders);
    let mut tallies: BTreeMap<PathBuf, Tally> = BTreeMap::new();
    for increment in &increments {
        if free.kib >= target {
            break;
        }
        let outcome = delete_idle(increment, before, open_files.get()?, free.run)?;
        let tally = tallies.entry(increment.profile.clone()).or_default();
        match outcome {
            Outcome::Deleted(bytes) => {
                tally.deleted += 1;
                tally.bytes += bytes;
                free.freed(bytes / 1024)?;
            }
            Outcome::Locked => tally.locked += 1,
            Outcome::Held => tally.held += 1,
            Outcome::Written => {}
        }
    }
    let verb = free.run.deleted();
    for (profile, tally) in &tallies {
        if tally.deleted > 0 {
            note(format!(
                "{verb} {} nothing wrote to in {FLOOR_INCREMENTAL_IDLE_MINUTES}m from {} ({})",
                counted(tally.deleted, "incremental folder", "incremental folders"),
                profile.display(),
                human_bytes(tally.bytes)
            ));
        }
        if tally.locked > 0 {
            note(format!(
                "kept {} in {}: a build holds its lock",
                counted(
                    tally.locked,
                    "idle incremental folder",
                    "idle incremental folders"
                ),
                profile.display()
            ));
        }
        if tally.held > 0 {
            note(format!(
                "kept {} in {}: a process holds their files",
                counted(
                    tally.held,
                    "idle incremental folder",
                    "idle incremental folders"
                ),
                profile.display()
            ));
        }
    }
    Ok(())
}

/// Brings the volume's free space up to the target, the floor plus its
/// margin, once <pruner> finds it under where that pruner starts (`Pruner`),
/// waiting for each deletion so the next free-space reading counts it: the
/// trash, the roots of checkouts that are gone, DerivedData's entries, every
/// checkout's idle incremental folders, then whole folders of the other
/// checkouts, each least recently written first and only when nothing is
/// using it or wrote to it lately. Says what it deleted and the free space
/// after. When everything left is in use, freshly written or kept and the
/// volume is still under the floor, it warns, naming the biggest folders, and
/// setup fails under the hard floor. A dry <run> deletes nothing and says
/// what it would delete.
pub fn keep_floor(tool: &Tool, roots: &[Root], pruner: Pruner, run: Run) -> Result<()> {
    let floor = tool.rules.floor_kib;
    let target = tool.rules.floor_target_kib();
    let start = tool.free_kib()?;
    let starts_under = match pruner {
        Pruner::Setup => floor,
        Pruner::Prune => target,
    };
    if start >= starts_under {
        return Ok(());
    }
    let under = if start < floor {
        format!("the {} floor", human(floor))
    } else {
        format!(
            "the {} floor plus its {} margin",
            human(floor),
            human(tool.rules.floor_margin_kib)
        )
    };
    let doing = if run.dry() {
        "a real run would delete"
    } else {
        "deleting"
    };
    note(format!(
        "the build volume has {} free, under {under}; {doing} idle DerivedData entries, then incremental folders nothing wrote to in {FLOOR_INCREMENTAL_IDLE_MINUTES}m, then idle build folders of unclaimed side and job worktrees, least recently written first, until it has {} free",
        human(start),
        human(target)
    ));
    let mut free = Free {
        tool,
        run,
        pruner,
        kib: start,
    };
    if !run.dry() {
        // Folders already renamed into the trash still hold their space until
        // the background removal finishes.
        crate::trash::empty_now(&tool.places)?;
        free.kib = tool.free_kib()?;
    }
    let gone: Vec<&Root> = roots.iter().filter(|root| root.live.is_none()).collect();
    let gone_paths: Vec<PathBuf> = gone.iter().map(|root| root.path.clone()).collect();
    let gone_measures = if free.kib >= target {
        Vec::new()
    } else {
        tool.measure_all(&gone_paths)?
    };
    for (root, measured) in gone.into_iter().zip(gone_measures) {
        if free.kib >= target {
            break;
        }
        let why = format!("{}; its checkout is gone", human(measured.kib));
        free.delete(&root.path, measured.kib, Recreate::No, &why)?;
    }
    let entries = if free.kib >= target {
        Vec::new()
    } else {
        derived_data(tool)?.1
    };
    for entry in entries {
        if free.kib >= target {
            break;
        }
        let measured = Measure {
            kib: entry.kib,
            newest: Some(entry.newest),
        };
        if tool.written_within(&measured, FLOOR_IDLE_MINUTES) {
            continue;
        }
        let why = format!(
            "{}, last write {}; a DerivedData entry",
            human(entry.kib),
            local_minute(entry.newest)
        );
        free.delete(&entry.path, entry.kib, Recreate::No, &why)?;
    }
    if free.kib < target {
        delete_idle_increments(tool, roots, &mut free, target)?;
    }
    for candidate in floor_candidates(tool, roots)? {
        if free.kib >= target {
            break;
        }
        if candidate.measured.kib <= 1024
            || tool.written_within(&candidate.measured, FLOOR_IDLE_MINUTES)
        {
            continue;
        }
        let why = format!(
            "{}, last write {}; its next build refills it",
            human(candidate.measured.kib),
            local_minute(candidate.measured.newest.unwrap_or(0))
        );
        free.delete(&candidate.path, candidate.measured.kib, Recreate::Yes, &why)?;
    }
    let free = free.kib;
    note(format!(
        "{} {}; the build volume {} {} free, against a target of {}",
        if run.dry() {
            "would free about"
        } else {
            "freed"
        },
        human(free.saturating_sub(start)),
        if run.dry() { "would have" } else { "has" },
        human(free),
        human(target)
    ));
    if free >= floor {
        return Ok(());
    }
    let hard = pruner == Pruner::Setup && free < tool.rules.hard_floor_kib;
    let mut lines = Vec::new();
    if hard {
        lines.push(format!(
            "builds: the build volume has {} free, under the {} hard floor, so this build stops before it starts.",
            human(free),
            human(tool.rules.hard_floor_kib)
        ));
    } else if pruner == Pruner::Setup {
        lines.push(format!(
            "builds: warning: the build volume has {} free, under the {} floor; the build goes on, and under {} builds stop.",
            human(free),
            human(floor),
            human(tool.rules.hard_floor_kib)
        ));
    } else {
        lines.push(format!(
            "builds: warning: the build volume {} {} free after this prune, under the {} floor; under {} builds stop.",
            if run.dry() { "would have" } else { "has" },
            human(free),
            human(floor),
            human(tool.rules.hard_floor_kib)
        ));
    }
    lines.push(format!(
        "Every incremental folder left was written in the last {FLOOR_INCREMENTAL_IDLE_MINUTES}m or is in use, and every build folder and DerivedData entry left is in use, was written in the last {FLOOR_IDLE_MINUTES}m, belongs to a claimed worktree, or is main's or an app checkout's own lane's, which stay warm. The biggest the floor may delete:"
    ));
    let mut biggest = floor_candidates(tool, roots)?;
    biggest.sort_by(|a, b| {
        b.measured
            .kib
            .cmp(&a.measured.kib)
            .then_with(|| b.path.cmp(&a.path))
    });
    for candidate in biggest.iter().take(8) {
        lines.push(format!(
            "    {}  {}",
            human(candidate.measured.kib),
            candidate.path.display()
        ));
    }
    let derived = tool.places.derived_data();
    let mut warm = Vec::new();
    for root in roots {
        if root.live.is_some() && root.keeps_warm(&tool.places)? {
            warm.push(root.path.clone());
        }
    }
    let caches: Vec<PathBuf> = SHARED_CACHES
        .iter()
        .map(|cache| tool.places.caches().join(cache.name))
        .filter(|dir| dir.is_dir())
        .collect();
    let mut asked = vec![derived.clone()];
    asked.extend(warm.iter().cloned());
    asked.extend(caches.iter().cloned());
    let mut kib = tool
        .measure_all(&asked)?
        .into_iter()
        .map(|measured| measured.kib);
    let derived_kib = kib.next().expect("measured");
    if derived.is_dir() {
        lines.push(format!(
            "    {}  {} (entries in use or written lately)",
            human(derived_kib),
            derived.display()
        ));
    }
    lines.push("Kept warm:".to_string());
    for path in &warm {
        lines.push(format!(
            "    {}  {}",
            human(kib.next().expect("measured")),
            path.display()
        ));
    }
    lines.push("Shared caches, which only their caps delete:".to_string());
    for dir in &caches {
        lines.push(format!(
            "    {}  {}",
            human(kib.next().expect("measured")),
            dir.display()
        ));
    }
    lines.push(
        "Retire finished side worktrees (builds retire), or delete idle build folders (builds prune --idle <minutes>)."
            .to_string(),
    );
    crate::output::warn(lines.join("\n"));
    if hard { Err(Fail::Exit(1)) } else { Ok(()) }
}

#[cfg(test)]
#[path = "floor_tests.rs"]
mod tests;

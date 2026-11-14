//! The free-space floor.
//!
//! Caps bound each folder, not the volume: a checkout with every platform
//! built is 45-52 GB, and the build volume shares its APFS pool with the main
//! disk. So `setup` also keeps a free-space floor: under it, it deletes
//! DerivedData's idle entries, then the incremental folders nothing wrote to
//! for FLOOR_INCREMENTAL_IDLE_MINUTES in every checkout, then whole idle
//! folders across every checkout but main and the app checkout, whose builds
//! stay warm on purpose, and but claimed worktrees, whose agents are
//! mid-task, least recently written first. If it still can't reach the floor
//! it warns, naming the biggest folders, and the build goes on: whoever runs
//! the agents frees space or runs fewer of them. Only under the hard floor
//! does it fail, rather than let the build run out of space halfway. It never
//! deletes a shared cache.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::checkout::Root;
use crate::evict::{Recreate, Removal, evict};
use crate::fail::{Fail, Result};
use crate::manifest::{SHARED_CACHES, build_dirs};
use crate::places::{FLOOR_IDLE_MINUTES, FLOOR_INCREMENTAL_IDLE_MINUTES};
use crate::prune::derived_data;
use crate::text::{counted, human, human_bytes, local_minute, note};
use crate::tool::Tool;
use crate::units::OpenFiles;
use crate::units::idle::{Outcome, delete_idle, idle_increments};
use crate::usage::Measure;

/// A build folder the floor may delete, with what it takes.
struct Candidate {
    path: PathBuf,
    measured: Measure,
}

/// The build folders the floor may delete, least recently written first:
/// every live checkout's but main's, a claimed one's and the folders an app
/// checkout's lane builds into.
/// A claimed worktree has an agent at work in it, who may go quiet for an
/// hour between builds; deleting its folders costs that agent a cold rebuild
/// of every dependency.
fn floor_candidates(tool: &Tool, roots: &[Root]) -> Result<Vec<Candidate>> {
    let mut paths = Vec::new();
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        if !checkout.is_side() || checkout.claim_file().is_file() {
            continue;
        }
        let lane = root.app_lane(&tool.places)?;
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
/// first, until the volume has <floor> free; says what went from each
/// profile folder and what stayed. Returns the free space after.
fn delete_idle_increments(tool: &Tool, roots: &[Root], mut free: u64, floor: u64) -> Result<u64> {
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
    let mut open_files = OpenFiles::new(&folders);
    let mut tallies: BTreeMap<PathBuf, Tally> = BTreeMap::new();
    for increment in &increments {
        if free >= floor {
            break;
        }
        let outcome = delete_idle(increment, before, open_files.get()?)?;
        let tally = tallies.entry(increment.profile.clone()).or_default();
        match outcome {
            Outcome::Deleted(bytes) => {
                tally.deleted += 1;
                tally.bytes += bytes;
                free = tool.free_kib()?;
            }
            Outcome::Locked => tally.locked += 1,
            Outcome::Held => tally.held += 1,
            Outcome::Written => {}
        }
    }
    for (profile, tally) in &tallies {
        if tally.deleted > 0 {
            note(format!(
                "deleted {} nothing wrote to in {FLOOR_INCREMENTAL_IDLE_MINUTES}m from {} ({})",
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
    Ok(free)
}

/// Brings the volume's free space up to the floor, waiting for each deletion
/// so the next free-space reading counts it: the trash, the roots of
/// checkouts that are gone, DerivedData's entries, every checkout's idle
/// incremental folders, then whole folders of the other checkouts, each least
/// recently written first and only when nothing is using it or wrote to it
/// lately. When everything left is in use, freshly written or kept, it warns,
/// naming the biggest folders, and fails only under the hard floor.
pub fn keep_floor(tool: &Tool, roots: &[Root]) -> Result<()> {
    let floor = tool.rules.floor_kib;
    let mut free = tool.free_kib()?;
    if free >= floor {
        return Ok(());
    }
    note(format!(
        "the build volume has {} free, under the {} floor; deleting idle DerivedData entries, then incremental folders nothing wrote to in {FLOOR_INCREMENTAL_IDLE_MINUTES}m, then idle build folders of unclaimed side worktrees, least recently written first",
        human(free),
        human(floor)
    ));
    // Folders already renamed into the trash still hold their space until
    // the background removal finishes.
    crate::trash::empty_now(&tool.places)?;
    free = tool.free_kib()?;
    let gone: Vec<&Root> = roots.iter().filter(|root| root.live.is_none()).collect();
    let gone_paths: Vec<PathBuf> = gone.iter().map(|root| root.path.clone()).collect();
    let gone_measures = if free >= floor {
        Vec::new()
    } else {
        tool.measure_all(&gone_paths)?
    };
    for (root, measured) in gone.into_iter().zip(gone_measures) {
        if free >= floor {
            break;
        }
        let kib = measured.kib;
        if evict(&tool.places, &root.path, Recreate::No, Removal::Now)? {
            note(format!(
                "deleted {} ({}; its checkout is gone)",
                root.path.display(),
                human(kib)
            ));
            free = tool.free_kib()?;
        }
    }
    let entries = if free >= floor {
        Vec::new()
    } else {
        derived_data(tool)?.1
    };
    for entry in entries {
        if free >= floor {
            break;
        }
        let measured = Measure {
            kib: entry.kib,
            newest: Some(entry.newest),
        };
        if tool.written_within(&measured, FLOOR_IDLE_MINUTES) {
            continue;
        }
        if evict(&tool.places, &entry.path, Recreate::No, Removal::Now)? {
            note(format!(
                "deleted {} ({}, last write {}), a DerivedData entry",
                entry.path.display(),
                human(entry.kib),
                local_minute(entry.newest)
            ));
            free = tool.free_kib()?;
        } else {
            note(format!("in use, kept: {}", entry.path.display()));
        }
    }
    if free < floor {
        free = delete_idle_increments(tool, roots, free, floor)?;
    }
    for candidate in floor_candidates(tool, roots)? {
        if free >= floor {
            break;
        }
        if candidate.measured.kib <= 1024
            || tool.written_within(&candidate.measured, FLOOR_IDLE_MINUTES)
        {
            continue;
        }
        if evict(&tool.places, &candidate.path, Recreate::Yes, Removal::Now)? {
            note(format!(
                "deleted {} ({}, last write {}); its next build refills it",
                candidate.path.display(),
                human(candidate.measured.kib),
                local_minute(candidate.measured.newest.unwrap_or(0))
            ));
            free = tool.free_kib()?;
        } else {
            note(format!("in use, kept: {}", candidate.path.display()));
        }
    }
    if free >= floor {
        return Ok(());
    }
    let hard = free < tool.rules.hard_floor_kib;
    let mut lines = Vec::new();
    if hard {
        lines.push(format!(
            "builds: the build volume has {} free, under the {} hard floor, so this build stops before it starts.",
            human(free),
            human(tool.rules.hard_floor_kib)
        ));
    } else {
        lines.push(format!(
            "builds: warning: the build volume has {} free, under the {} floor; the build goes on, and under {} builds stop.",
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
    eprintln!("{}", lines.join("\n"));
    if hard { Err(Fail::Exit(1)) } else { Ok(()) }
}

#[cfg(test)]
#[path = "floor_tests.rs"]
mod tests;

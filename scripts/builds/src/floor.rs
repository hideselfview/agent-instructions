//! The free-space floor.
//!
//! Caps bound each folder, not the volume: a checkout with every platform
//! built is 45-52 GB, and the build volume shares its APFS pool with the main
//! disk. So `setup` also keeps a free-space floor: under it, it deletes
//! DerivedData's idle entries, then whole idle folders across every checkout
//! but main and the app checkout, whose builds stay warm on purpose, and but
//! claimed worktrees, whose agents are mid-task, least recently written
//! first. If it still can't reach the floor it warns, naming the biggest
//! folders, and the build goes on: whoever runs the agents frees space or runs
//! fewer of them. Only under the hard floor does it fail, rather than let the
//! build run out of space halfway. It never deletes a shared cache.

use std::path::PathBuf;

use crate::checkout::Root;
use crate::evict::{Recreate, Removal, evict};
use crate::fail::{Fail, Result};
use crate::manifest::{SHARED_CACHES, build_dirs};
use crate::places::FLOOR_IDLE_MINUTES;
use crate::prune::derived_data_entries;
use crate::text::{human, local_minute, note};
use crate::tool::Tool;
use crate::usage::Measure;

/// A build folder the floor may delete, with what it takes.
struct Candidate {
    path: PathBuf,
    measured: Measure,
}

/// The build folders the floor may delete, least recently written first:
/// every live checkout's but main's, the app checkout's and a claimed one's.
/// A claimed worktree has an agent at work in it, who may go quiet for an
/// hour between builds; deleting its folders costs that agent a cold rebuild
/// of every dependency.
fn floor_candidates(tool: &Tool, roots: &[Root]) -> Result<Vec<Candidate>> {
    let mut paths = Vec::new();
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        if root.keeps_warm(&tool.places)? || checkout.claim_file().is_file() {
            continue;
        }
        for entry in build_dirs(&checkout.top)? {
            let folder = root.path.join(&entry.path);
            if folder.is_dir() {
                paths.push(folder);
            }
        }
    }
    let measured = crate::usage::measure_many(&paths)?;
    let mut candidates: Vec<Candidate> =
        paths.into_iter().zip(measured).map(|(path, measured)| Candidate { path, measured }).collect();
    candidates.sort_by(|a, b| {
        a.measured.newest.unwrap_or(0).cmp(&b.measured.newest.unwrap_or(0)).then_with(|| a.path.cmp(&b.path))
    });
    Ok(candidates)
}

/// Brings the volume's free space up to the floor, waiting for each deletion
/// so the next free-space reading counts it: the trash, the roots of
/// checkouts that are gone, DerivedData's entries, then whole folders of the
/// other checkouts, each least recently written first and only when nothing
/// is using it or wrote to it lately. When everything left is in use, freshly
/// written or kept, it warns, naming the biggest folders, and fails only under
/// the hard floor.
pub fn keep_floor(tool: &Tool, roots: &[Root]) -> Result<()> {
    let floor = tool.rules.floor_kib;
    let mut free = tool.free_kib()?;
    if free >= floor {
        return Ok(());
    }
    note(format!(
        "the build volume has {} free, under the {} floor; deleting idle DerivedData entries, then idle build folders of unclaimed side worktrees, least recently written first",
        human(free),
        human(floor)
    ));
    // Folders already renamed into the trash still hold their space until
    // the background removal finishes.
    let trash = tool.places.trash();
    if trash.is_dir() {
        if let Err(error) = std::fs::remove_dir_all(&trash) {
            note(format!("emptying {} failed partway: {error}", trash.display()));
        }
        free = tool.free_kib()?;
    }
    for root in roots {
        if free >= floor {
            break;
        }
        if root.live.is_some() {
            continue;
        }
        let kib = tool.measure(&root.path)?.kib;
        if evict(&tool.places, &root.path, Recreate::No, Removal::Now)? {
            note(format!("deleted {} ({}; its checkout is gone)", root.path.display(), human(kib)));
            free = tool.free_kib()?;
        }
    }
    for entry in derived_data_entries(tool)? {
        if free >= floor {
            break;
        }
        let measured = Measure { kib: entry.kib, newest: Some(entry.newest) };
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
    for candidate in floor_candidates(tool, roots)? {
        if free >= floor {
            break;
        }
        if candidate.measured.kib <= 1024 || tool.written_within(&candidate.measured, FLOOR_IDLE_MINUTES) {
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
        "Every build folder and DerivedData entry left is in use, was written in the last {FLOOR_IDLE_MINUTES}m, belongs to a claimed worktree, or is main's or an app checkout's, which stay warm. The biggest the floor may delete:"
    ));
    let mut biggest = floor_candidates(tool, roots)?;
    biggest.sort_by(|a, b| b.measured.kib.cmp(&a.measured.kib).then_with(|| b.path.cmp(&a.path)));
    for candidate in biggest.iter().take(8) {
        lines.push(format!("    {}  {}", human(candidate.measured.kib), candidate.path.display()));
    }
    let derived_data = tool.places.derived_data();
    if derived_data.is_dir() {
        lines.push(format!(
            "    {}  {} (entries in use or written lately)",
            human(tool.measure(&derived_data)?.kib),
            derived_data.display()
        ));
    }
    lines.push("Kept warm:".to_string());
    for root in roots {
        if root.live.is_some() && root.keeps_warm(&tool.places)? {
            lines.push(format!("    {}  {}", human(tool.measure(&root.path)?.kib), root.path.display()));
        }
    }
    lines.push("Shared caches, which only their caps delete:".to_string());
    for cache in SHARED_CACHES {
        let dir = tool.places.caches().join(cache.name);
        if dir.is_dir() {
            lines.push(format!("    {}  {}", human(tool.measure(&dir)?.kib), dir.display()));
        }
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

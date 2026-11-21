//! How health reads a running `builds land`: which worktree it lands in, and
//! which other landing holds a turn that this one takes (land.rs, `take_turn`).

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::land::{APP_TURN_FILE, WORKTREE_TURN_FILE, git_text};

/// Returns the worktree that the command line <argv> of a running `builds
/// land <worktree> ...` lands in. Returns None for any other command.
pub fn requested(argv: &[String]) -> Option<&str> {
    match argv {
        [_, command, worktree, ..] if command == "land" && !worktree.starts_with('-') => {
            Some(worktree.as_str())
        }
        _ => None,
    }
}

/// What the landing with process id <pid> in <worktree> waits for: the
/// landing of another process that holds the turn of landings in the
/// worktree, or the turn of landings that change the repo's apps. None when
/// no other landing holds either turn. A landing that has not yet reached the
/// apps turn is also told of the landing that holds it, since it takes that
/// turn before its fetch.
pub fn waits_for(worktree: &Path, pid: i32) -> Option<String> {
    let git_dir = git_text(worktree, &["rev-parse", "--absolute-git-dir"])?;
    let common = git_text(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    [
        (
            PathBuf::from(git_dir).join(WORKTREE_TURN_FILE),
            "the landing in this worktree",
        ),
        (
            PathBuf::from(common).join(APP_TURN_FILE),
            "the landing that changes the apps",
        ),
    ]
    .into_iter()
    .find_map(|(file, whose)| {
        held_by_another(&file, pid).map(|holder| format!("{whose}, {holder}"))
    })
}

/// The landing that holds the lock of the turn file <path>, as it wrote
/// itself there ("process 7 landing ..."), when it is not process <pid>.
/// None when no process holds the lock or <pid> holds it.
fn held_by_another(path: &Path, pid: i32) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    if crate::flock::try_lock(&file, libc::LOCK_SH, path).ok()? {
        return None;
    }
    let mut holder = String::new();
    file.read_to_string(&mut holder).ok()?;
    let holder = holder.trim();
    let holder_pid = holder
        .strip_prefix("process ")
        .and_then(|rest| rest.split(' ').next())
        .and_then(|word| word.parse::<i32>().ok());
    if holder_pid == Some(pid) {
        return None;
    }
    Some(if holder.is_empty() {
        "held by another landing".to_string()
    } else {
        holder.to_string()
    })
}

#[cfg(test)]
#[path = "land_turns_tests.rs"]
mod tests;

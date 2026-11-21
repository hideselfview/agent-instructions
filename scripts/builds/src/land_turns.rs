//! How health reads a running `builds land`: which worktree it lands in, and
//! which other landing holds a turn that this one takes (land.rs, `take_turn`).

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Fail, Result};
use crate::land::{APP_TURN_FILE, WORKTREE_TURN_FILE, git_text, read_holder, record_pid};

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
pub fn waits_for(worktree: &Path, pid: i32) -> Result<Option<String>> {
    let git = |args: &[&str]| {
        git_text(worktree, args).ok_or_else(|| {
            Fail::Message(format!(
                "git {} failed in {}",
                args.join(" "),
                worktree.display()
            ))
        })
    };
    let git_dir = git(&["rev-parse", "--absolute-git-dir"])?;
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    for (file, whose) in [
        (
            PathBuf::from(git_dir).join(WORKTREE_TURN_FILE),
            "the landing in this worktree",
        ),
        (
            PathBuf::from(common).join(APP_TURN_FILE),
            "the landing that changes the apps",
        ),
    ] {
        if let Some(holder) = held_by_another(&file, pid)? {
            return Ok(Some(format!("{whose}, {holder}")));
        }
    }
    Ok(None)
}

/// The landing that holds the lock of the turn file <path>, as it wrote
/// itself there, when it is not process <pid>. None when no process holds the
/// lock, the file does not exist or <pid> holds it.
fn held_by_another(path: &Path, pid: i32) -> Result<Option<String>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("opening {}: {error}", path.display()),
    };
    if crate::flock::try_lock(&file, libc::LOCK_SH, path)? {
        return Ok(None);
    }
    let holder = read_holder(&mut file, path)?;
    if record_pid(&holder) == Some(pid) {
        return Ok(None);
    }
    Ok(Some(if holder.is_empty() {
        "held by another landing".to_string()
    } else {
        holder
    }))
}

#[cfg(test)]
#[path = "land_turns_tests.rs"]
mod tests;

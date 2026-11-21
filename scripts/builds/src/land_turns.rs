//! How health reads a running `builds land`: which worktree it lands in, and
//! which other landing holds a turn that this one takes (land.rs, `take_turn`).

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::Result;
use crate::land::{WORKTREE_TURN_FILE, app_turn_file, read_holder, record_pid};

/// Returns the worktree that the command line <argv> of a running `builds
/// land <worktree> ...` lands in. Returns None for any other command.
pub fn requested(argv: &[String]) -> Option<&str> {
    match crate::ack::agent::without_flag(argv)[..] {
        [_, "land", worktree, ..] if !worktree.starts_with('-') => Some(worktree),
        _ => None,
    }
}

/// The turn a landing waits for and the landing that holds it, as that
/// landing wrote itself into the turn file.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnWait {
    pub turn: String,
    pub holder: String,
}

impl std::fmt::Display for TurnWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}, {}", self.turn, self.holder)
    }
}

/// The worktree that the running process <pid>, whose command line is <argv>,
/// lands in: None when it is no `builds land`, and the text of what failed
/// when builds cannot read its working directory.
pub fn landing_worktree(pid: i32, argv: &[String]) -> Option<std::result::Result<PathBuf, String>> {
    let worktree = requested(argv)?;
    Some(
        crate::procs::cwd(pid)
            .map(|dir| dir.join(worktree))
            .ok_or_else(|| {
                format!(
                    "builds cannot read the working directory of process {pid}, a `builds land`, so health cannot tell which worktree it lands in"
                )
            }),
    )
}

/// The commits that the command line <argv> of a running `builds land
/// <worktree> ...` lands, read as `builds land` reads them.
fn landed_commits(argv: &[String]) -> Vec<String> {
    let args: Vec<String> = crate::ack::agent::without_flag(argv)
        .into_iter()
        .skip(3)
        .map(str::to_string)
        .collect();
    crate::land::commits_named(&args).unwrap_or_default()
}

/// What the landing with process id <pid> and command line <argv> in
/// <worktree> waits for: the landing of another process that holds the turn
/// of landings in the worktree, or the turn of landings that change the
/// apps of a lane that this landing changes. None when no other landing
/// holds a turn it takes. A landing that has not yet reached its app turns is
/// also told of the landing that holds one, since it takes them before its
/// fetch.
pub fn waits_for(worktree: &Path, pid: i32, argv: &[String]) -> Result<Option<TurnWait>> {
    let git = |args: &[&str]| {
        crate::review::commits::git_text(worktree, args).map(|text| text.trim().to_string())
    };
    let git_dir = git(&["rev-parse", "--absolute-git-dir"])?;
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let mut turns = vec![(
        PathBuf::from(git_dir).join(WORKTREE_TURN_FILE),
        "the landing in this worktree".to_string(),
    )];
    let commits = landed_commits(argv);
    if !commits.is_empty() {
        for lane in crate::app_landing::lanes_changed(worktree, &commits)? {
            turns.push((
                PathBuf::from(&common).join(app_turn_file(lane)),
                format!("the landing that changes the {} apps", lane.word()),
            ));
        }
    }
    for (file, turn) in turns {
        if let Some(holder) = held_by_another(&file, pid)? {
            return Ok(Some(TurnWait { turn, holder }));
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

//! How health reads a running `builds land`: which worktree it lands in, and
//! whether another landing holds the turn of landings in that worktree (land.rs,
//! `take_turn`).

use std::path::{Path, PathBuf};

use crate::command::{Command, Parsed};
use crate::fail;
use crate::fail::Result;
use crate::land::{WORKTREE_TURN_FILE, read_holder, record_branch, record_pid};

/// Returns the worktree that the command line <argv> of a running `builds
/// land <worktree> ...` lands in. Returns None for any other command.
pub fn requested(argv: &[String]) -> Option<&str> {
    let words = crate::ack::agent::without_flag(argv);
    match Command::of(words.get(1..)?)? {
        Parsed {
            command: Command::Land,
            rest: [worktree, ..],
            ..
        } if !worktree.starts_with('-') => Some(worktree),
        _ => None,
    }
}

/// The turn a landing waits for and the landing that holds it, as that
/// landing wrote itself into the turn file.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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

/// What the turn file of a worktree says of the landing with process id
/// <pid> there.
#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeTurn {
    /// The landing of another process that holds the turn of landings in the
    /// worktree. None when no other landing holds it.
    pub waits: Option<TurnWait>,
    /// The branch that the landing holding the turn started on: <pid>'s own,
    /// or the one <pid> waits for. While that landing runs, HEAD may be on
    /// another commit.
    pub branch: Option<String>,
}

/// Reads the turn of landings in <worktree> for the landing <pid>.
pub fn read_turn(worktree: &Path, pid: i32) -> Result<WorktreeTurn> {
    let git_dir = crate::git::text(worktree, &["rev-parse", "--absolute-git-dir"])
        .map(|text| text.trim().to_string())?;
    turn_in(&PathBuf::from(git_dir).join(WORKTREE_TURN_FILE), pid)
}

/// `read_turn` of the turn file <path>.
fn turn_in(path: &Path, pid: i32) -> Result<WorktreeTurn> {
    let Some(holder) = held(path)? else {
        return Ok(WorktreeTurn {
            waits: None,
            branch: None,
        });
    };
    let branch = record_branch(&holder).map(str::to_string);
    let waits = (record_pid(&holder) != Some(pid)).then(|| TurnWait {
        turn: "the landing in this worktree".to_string(),
        holder: if holder.is_empty() {
            "held by another landing".to_string()
        } else {
            holder
        },
    });
    Ok(WorktreeTurn { waits, branch })
}

/// What the landing that holds the lock of the turn file <path> wrote there.
/// None when no process holds the lock or the file does not exist.
fn held(path: &Path) -> Result<Option<String>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("opening {}: {error}", path.display()),
    };
    if crate::flock::try_lock(&file, libc::LOCK_SH, path)? {
        return Ok(None);
    }
    Ok(Some(read_holder(&mut file, path)?))
}

#[cfg(test)]
#[path = "land_turns_tests.rs"]
mod tests;

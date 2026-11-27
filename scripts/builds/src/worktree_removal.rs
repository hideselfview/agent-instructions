//! Removing a worktree whole. `git worktree remove` does not: when it cannot
//! delete one file, it still deletes the worktree's git dir, which leaves a
//! folder with some files gone and a .git file that points at nothing.
//! `remove` renames two folders into a new folder, the entry
//! <main>/.git/builds-retiring/<name>.XXXXXXXX, and deletes files only after
//! both renames, neither of which can happen in part:
//!
//! 1. the worktree's folder, to <entry>/files
//! 2. its git dir, which is how git knows the worktree, to <entry>/git
//!
//! The second rename removes the worktree. Until then builds moves the folder
//! back: `remove` does when that rename fails, and `finish` does for a
//! process that was killed between the two. After it nothing references the
//! entry, and builds deletes it. Every caller of `remove` and `finish` holds
//! the repo's picking lock (`worktree::lock_picking`, which calls `finish`),
//! so no entry is another process's removal in progress.

use std::path::{Path, PathBuf};

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::output::note;

/// The folder of <main>'s repo that holds the entries.
fn entries_folder(main: &Path) -> PathBuf {
    main.join(".git/builds-retiring")
}

/// Is there something at <path>? Fails when builds cannot tell.
fn present(path: &Path) -> Result<bool> {
    path.try_exists()
        .context(|| format!("reading {}", path.display()))
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to).context(|| format!("moving {} to {}", from.display(), to.display()))
}

/// Removes the worktree <checkout>, its files and git's record of it, or
/// fails and leaves it as it was. Refuses one that git has locked and one
/// whose git dir holds submodules, whose commits may be nowhere else, as
/// `git worktree remove` does. The caller holds the picking lock and has
/// judged that the worktree's files can go.
pub fn remove(checkout: &Checkout) -> Result<()> {
    let top = &checkout.top;
    for (name, why) in [
        (
            "locked",
            "git has it locked (`git worktree unlock` unlocks it)",
        ),
        ("modules", "its git dir holds submodules"),
    ] {
        if present(&checkout.git_dir.join(name))? {
            fail!("builds cannot remove {}: {why}", top.display());
        }
    }
    let folder = entries_folder(&checkout.main);
    std::fs::create_dir_all(&folder).context(|| format!("creating {}", folder.display()))?;
    let entry = crate::evict::make_temp_dir(&folder, &format!("{}.", checkout.name()))?;
    let files = entry.join("files");
    if let Err(error) = rename(top, &files) {
        return Err(also(error, remove_empty(&entry)));
    }
    if let Err(error) = rename(&checkout.git_dir, &entry.join("git")) {
        // When this fails too, the entry keeps the files, and `finish` moves
        // them back.
        let undone = rename(&files, top).and_then(|()| remove_empty(&entry));
        return Err(also(error, undone));
    }
    delete(&entry);
    Ok(())
}

/// <error>, why a removal failed, with the failure of the <undone> steps
/// that followed it, when they failed too.
fn also(error: Fail, undone: Result<()>) -> Fail {
    match undone {
        Ok(()) => error,
        Err(then) => Fail::Message(format!("{}; then {}", error.text(), then.text())),
    }
}

/// Ends each removal of a worktree of <main>'s repo that an earlier process
/// left: moves the folder back when git still knows the worktree, and deletes
/// the entry of a removed one.
pub fn finish(main: &Path) -> Result<()> {
    let folder = entries_folder(main);
    let listed = match std::fs::read_dir(&folder) {
        Ok(listed) => listed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => fail!("listing {}: {error}", folder.display()),
    };
    for found in listed {
        let entry = found
            .context(|| format!("listing {}", folder.display()))?
            .path();
        let files = entry.join("files");
        if present(&entry.join("git"))? {
            delete(&entry);
        } else if present(&files)? {
            let named = crate::checkout::file_name(&entry);
            let (Some((name, _)), Some(worktrees)) = (
                named.rsplit_once('.'),
                crate::checkout::worktrees_folder(main),
            ) else {
                fail!(
                    "builds cannot tell which worktree {} holds",
                    entry.display()
                );
            };
            let top = worktrees.join(name);
            if present(&top)? {
                fail!(
                    "a removal of the worktree {top} stopped before git forgot it, and its files are in {files}, but {top} exists again; move one of the two away",
                    top = top.display(),
                    files = files.display()
                );
            }
            rename(&files, &top)?;
            remove_empty(&entry)?;
            note(format!(
                "moved {} back: its removal stopped before git forgot it",
                top.display()
            ));
        } else {
            delete(&entry);
        }
    }
    Ok(())
}

fn remove_empty(entry: &Path) -> Result<()> {
    std::fs::remove_dir(entry).context(|| format!("removing {}", entry.display()))
}

/// Deletes the entry of a removed worktree, its files first: `finish` reads
/// an entry with files and no git dir as a worktree to move back, so the git
/// dir stays until every file is gone. A failure leaves only bytes nothing
/// references, which the next `finish` deletes, so it fails no caller.
fn delete(entry: &Path) {
    let gone = |result: std::io::Result<()>| match result {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    };
    let deleted = gone(std::fs::remove_dir_all(entry.join("files")))
        .and_then(|()| gone(std::fs::remove_dir_all(entry)));
    if let Err(error) = deleted {
        note(format!(
            "warning: could not delete all of the removed worktree in {}: {error}; the next command that picks or retires a worktree of the repo tries again",
            entry.display()
        ));
    }
}

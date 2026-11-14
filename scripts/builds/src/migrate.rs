//! Moves from an older shape of what the tool stores, done in place by
//! `builds install`.
//!
//! The first is the move from a lane's one app checkout to a set of them, done in place by
//! `builds install`: each repo's `app-checkout` file becomes `app-checkouts`
//! and its `app.lock` becomes `app.locks/<checkout>` (a hard link to the same
//! file, so the new path and the old are one lock), likewise for the android
//! lane's `app-android-*`. The queue keeps its folder and format.
//!
//! A `builds` from before the move reads the old names, and a build running
//! under it holds the old lock or waits in the old queue. The move does not
//! wait for those builds and does not interrupt them: they and the new `builds`
//! share the lock (one file under two names) and the queue, so they take turns
//! as before. The old names are removed once nothing uses them, which is the
//! first move run after the builds end (`builds install` runs it each time).

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rusqlite::Connection;

use crate::db::{self, DATABASE, Sql};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::queue::is_held;

/// Moves every repo's old-layout lane files to the new layout; does nothing
/// for a lane already moved.
pub fn migrate_app_files(places: &Places) -> Result<()> {
    let repos = match std::fs::read_dir(&places.root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => fail!("listing {}: {error}", places.root.display()),
    };
    for repo in repos {
        let repo = repo.context(|| format!("listing {}", places.root.display()))?;
        let name = repo.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !repo.path().is_dir() {
            continue;
        }
        for lane in Lane::ALL {
            migrate_lane(places, &name, lane)?;
        }
    }
    Ok(())
}

fn migrate_lane(places: &Places, repo: &str, lane: Lane) -> Result<()> {
    let dir = places.root.join(repo);
    let old_file = dir.join(lane.legacy_checkout_file_name());
    let old_lock = dir.join(lane.legacy_lock_file_name());
    if !old_file.exists() && !old_lock.exists() {
        return Ok(());
    }
    if old_file.is_file() {
        let text = std::fs::read_to_string(&old_file)
            .context(|| format!("reading {}", old_file.display()))?;
        let checkout = text.trim().to_string();
        if checkout.is_empty() || checkout.contains('/') {
            fail!("{} names no side worktree ({text:?})", old_file.display());
        }
        // The old lock must exist before it is linked: a build from before
        // may be waiting to create it, and would make a second file.
        if !old_lock.exists() {
            crate::queue::open_with(&old_lock, libc::O_CREAT | libc::O_CLOEXEC)
                .map(|fd| unsafe { libc::close(fd) })
                .context(|| format!("creating {}", old_lock.display()))?;
        }
        let locks = places.app_locks_dir(repo, lane);
        std::fs::create_dir_all(&locks).context(|| format!("creating {}", locks.display()))?;
        let new_lock = locks.join(&checkout);
        if new_lock.exists() {
            let (old, new) = (std::fs::metadata(&old_lock), std::fs::metadata(&new_lock));
            let same = matches!((old, new), (Ok(old), Ok(new)) if old.ino() == new.ino() && old.dev() == new.dev());
            if !same {
                fail!(
                    "{} and {} are different files; remove the old one",
                    old_lock.display(),
                    new_lock.display()
                );
            }
        } else {
            std::fs::hard_link(&old_lock, &new_lock)
                .context(|| format!("linking {} to {}", new_lock.display(), old_lock.display()))?;
        }
        if !places.app_checkouts_file(repo, lane).exists() {
            places.write_app_checkouts(repo, lane, &[checkout])?;
        }
    }
    let queue = dir.join(lane.queue_dir_name());
    if (old_lock.exists() && is_held(&old_lock)?) || waiting_in(&queue)? {
        crate::text::note(format!(
            "{repo}'s {} lane has builds from before the move running or waiting; its old file names go the next time install runs with none",
            lane.word()
        ));
        return Ok(());
    }
    for old in [&old_lock, &old_file] {
        match std::fs::remove_file(old) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                fail!("removing {}: {error}", old.display())
            }
            _ => {}
        }
    }
    crate::text::note(format!(
        "moved {repo}'s {} app checkout to {}",
        lane.word(),
        places.app_checkouts_file(repo, lane).display()
    ));
    Ok(())
}

/// Does a process hold a ticket in the queue folder <queue>?
fn waiting_in(queue: &Path) -> Result<bool> {
    let entries = match std::fs::read_dir(queue) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => fail!("listing {}: {error}", queue.display()),
    };
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", queue.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with("order") {
            continue;
        }
        if is_held(&entry.path())? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The stamp of the schema before `hook_runs.base` was added.
const BEFORE_HOOK_BASE_STAMP: i64 = 119_742_261;

/// Gives each project's `builds.sqlite` the `hook_runs.base` column: a store
/// stamped as the schema before it gets the column in one transaction, is
/// stamped as today's schema, and must then equal a fresh database's schema
/// or the transaction rolls back. A store already at today's stamp, or with
/// no database, is left alone; any other stamp is refused. Delete this once it
/// has run.
pub fn migrate_hook_base(places: &Places) -> Result<()> {
    let entries = match std::fs::read_dir(&places.lists) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => fail!("listing {}: {error}", places.lists.display()),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", places.lists.display()))?;
        if entry.path().is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    for dir in dirs {
        add_hook_base(&dir.join(DATABASE))?;
    }
    Ok(())
}

fn add_hook_base(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let mut conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .context(|| format!("opening {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(i32::MAX as u64))
        .sql()?;
    let found = db::stamp_of(&conn, path)?;
    if found == db::stamp() || found == 0 {
        return Ok(());
    }
    if found != BEFORE_HOOK_BASE_STAMP {
        fail!(
            "{} carries stamp {found}, which this builds does not know how to move (it knows {BEFORE_HOOK_BASE_STAMP}, and today's {})",
            path.display(),
            db::stamp()
        );
    }
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .sql()?;
    tx.execute_batch("ALTER TABLE hook_runs ADD COLUMN base TEXT CHECK (base <> '')")
        .sql()?;
    tx.pragma_update(None, "user_version", db::stamp()).sql()?;
    if db::schema_rows(&tx)? != db::schema_rows(&db::memory())? {
        fail!(
            "{} holds a schema that is not the one this builds makes; it is left as it was",
            path.display()
        );
    }
    tx.commit().sql()?;
    crate::text::note(format!("gave {}'s hook runs a base commit", path.display()));
    Ok(())
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;

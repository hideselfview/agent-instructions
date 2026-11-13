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

use serde_json::{Value, json};

use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::lists::Stamp;
use crate::lists::store::stores;
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
        let text = std::fs::read_to_string(&old_file).context(|| format!("reading {}", old_file.display()))?;
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
                fail!("{} and {} are different files; remove the old one", old_lock.display(), new_lock.display());
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
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => fail!("removing {}: {error}", old.display()),
            _ => {}
        }
    }
    crate::text::note(format!("moved {repo}'s {} app checkout to {}", lane.word(), places.app_checkouts_file(repo, lane).display()));
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

/// The second and third moves, made together in one write of each store (the
/// state is read as the current shape only after both, so a store old in both
/// ways still moves):
///
/// The second: design changes recorded with an image of each board
/// (`render`) and no worktree or review page. Each, open or landed, gains its
/// worktree and a review page, and its boards lose the image.
///
/// A record never said which worktree its boards were drawn in, so it gets
/// the main checkout of its repo, `~/dev/<repo>`. Boards that are not there
/// show their review page's board as that problem until `design amend N`
/// from the worktree that holds them records the right one. The review page is
/// one section headed by the change's title, with every board of the change in
/// the dark, captioned with its file name.
///
/// The third: an item's `running`, once one worktree or null, is a list with
/// one entry per repo the work spans. An object becomes a one-entry list and
/// null an empty one, for open items and for items in the closed records.
pub fn migrate_lists(places: &Places, now: i64) -> Result<()> {
    for store in stores(places)? {
        let stamp = Stamp { at: now, by: "builds install".to_string() };
        let moved = store.migrate(&stamp, "migrate lists to the current shape", |state| {
            let mut moved = false;
            let dev = places.home.join("dev");
            if let Some(open) = state.get_mut("designs").and_then(Value::as_array_mut) {
                for design in open {
                    moved |= migrate_design(design, &dev)?;
                }
            }
            if let Some(closed) = state.get_mut("closed").and_then(Value::as_array_mut) {
                for design in closed.iter_mut().filter_map(|c| c.pointer_mut("/record/design")) {
                    moved |= migrate_design(design, &dev)?;
                }
            }
            if let Some(open) = state.get_mut("items").and_then(Value::as_array_mut) {
                for item in open {
                    moved |= migrate_running(item)?;
                }
            }
            if let Some(closed) = state.get_mut("closed").and_then(Value::as_array_mut) {
                for item in closed.iter_mut().filter_map(|c| c.pointer_mut("/record/item")) {
                    moved |= migrate_running(item)?;
                }
            }
            Ok(moved)
        })?;
        if moved {
            crate::text::note(format!("moved project {}'s lists to the current shape", store.project));
        }
    }
    Ok(())
}

/// Moves one item's `running` to a list; false when it already is one.
fn migrate_running(item: &mut Value) -> Result<bool> {
    let Some(item) = item.as_object_mut() else { fail!("an item is not an object") };
    let Some(running) = item.get_mut("running") else { return Ok(false) };
    match running {
        Value::Array(_) => Ok(false),
        Value::Null => {
            *running = json!([]);
            Ok(true)
        }
        Value::Object(_) => {
            *running = json!([running.take()]);
            Ok(true)
        }
        other => fail!("an item's running is {other}, neither null, an object nor a list"),
    }
}

/// Moves one design change's JSON to the current shape; false when it
/// already is. <dev> holds the repos' main checkouts.
fn migrate_design(design: &mut Value, dev: &Path) -> Result<bool> {
    let Some(record) = design.as_object_mut() else { fail!("a design change is not an object") };
    let mut moved = false;
    if let Some(boards) = record.get_mut("boards").and_then(Value::as_object_mut) {
        for board in boards.values_mut().filter_map(Value::as_object_mut) {
            moved |= board.remove("render").is_some();
        }
    }
    if !record.contains_key("worktree") {
        let Some(repo) = record.get("repo").and_then(Value::as_str) else { fail!("a design change has no repo") };
        let worktree = dev.join(repo).display().to_string();
        record.insert("worktree".to_string(), json!(worktree));
        moved = true;
    }
    if !record.contains_key("review") {
        let title = record.get("title").and_then(Value::as_str).unwrap_or("Boards").to_string();
        let boards: Vec<Value> = record
            .get("boards")
            .and_then(Value::as_object)
            .map(|boards| {
                boards
                    .keys()
                    .map(|path| {
                        let file = path.rsplit('/').next().unwrap_or(path);
                        let name = file.strip_suffix(".dc.html").unwrap_or(file);
                        json!({ "board": path, "caption": name, "theme": "dark" })
                    })
                    .collect()
            })
            .unwrap_or_default();
        record.insert("review".to_string(), json!([{ "heading": title, "words": "", "boards": boards }]));
        moved = true;
    }
    Ok(moved)
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;

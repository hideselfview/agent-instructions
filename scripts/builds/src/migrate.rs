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
use serde_json::Value;

use crate::db::{self, DATABASE, Sql};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lane::Lane;
use crate::lists::store;
use crate::lists::{self, Lists, LogLine};
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

/// The move of the lists from `lists.json` into `builds.sqlite`, made by
/// `builds install` for each project folder that still holds the JSON (or a
/// move a crash left half done).
///
/// The JSON is read as today's shape and nothing older. Everything the
/// database will derive is checked to agree with what the JSON stores twice,
/// and a copy of the whole store is built in memory and checked before
/// anything on disk changes, so a store that can't move is refused with
/// everything as it was. Then, holding `lists.lock` so no old binary is
/// mid-write:
///
/// 1. `lists.json` is renamed to `lists.json.moving`: from then on an old
///    binary fails loudly instead of writing JSON the database would never
///    see, and a new one refuses with "a move is under way".
/// 2. The database is brought to the current schema (a new file gets all of
///    it; one holding only the hook stats' tables gets the lists' tables, and
///    must then equal a fresh schema) and every row goes in one transaction,
///    with the move's own line in the write log.
/// 3. `lists.json.moving` is renamed to `lists.json.moved`, and `lists.lock`
///    goes.
///
/// Running install again finishes whatever state a crash left: `.moving` with
/// no lists rows moves again; `.moving` with the lists rows of its own move
/// only renames; anything else is refused naming what it found.
pub fn migrate_lists(places: &Places, now: i64) -> Result<()> {
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
        move_folder(&dir, now)?;
    }
    Ok(())
}

/// The stamp of the schema before `hook_runs.base` was added.
const BEFORE_HOOK_BASE_STAMP: i64 = 119_742_261;

/// Gives each project's `builds.sqlite` the `hook_runs.base` column: a store
/// stamped as the schema before it gets the column in one transaction, is
/// stamped as today's schema, and must then equal a fresh database's schema
/// or the transaction rolls back. A store already at today's stamp, or with
/// no database, is left alone; any other stamp is refused. Delete this once it
/// has run. Run it before `migrate_lists`, which refuses the stamp this one
/// moves.
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
    if found == db::stamp() || found == 0 || found == db::HOOK_ONLY_STAMP {
        // Today's schema already; or one `migrate_lists` makes whole.
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

/// What the move's own write log line says.
const MOVE_WORDS: &str = "move lists.json into builds.sqlite";
const MOVE_BY: &str = "builds install";

/// An exclusive lock on a folder's `lists.lock`, held until dropped: every old
/// binary's write holds it for the whole write.
struct Locked {
    _file: std::fs::File,
}

fn take_lock(path: &Path) -> Result<Locked> {
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .context(|| format!("opening {}", path.display()))?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(Locked { _file: file });
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            fail!("locking {}: {error}", path.display());
        }
    }
}

/// How many lists rows the database in <dir> holds, and its latest version:
/// None when it holds no project.
fn lists_rows(dir: &Path) -> Result<Option<u64>> {
    let path = dir.join(DATABASE);
    if !path.is_file() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context(|| format!("opening {}", path.display()))?;
    let stamp = db::stamp_of(&conn, &path)?;
    if stamp == 0 || stamp == db::HOOK_ONLY_STAMP {
        return Ok(None);
    }
    if stamp != db::stamp() {
        fail!(
            "{} was made by another builds schema (it carries {stamp})",
            path.display()
        );
    }
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
        .sql()?;
    if rows == 0 {
        return Ok(None);
    }
    Ok(Some(store::version(&conn)?))
}

fn move_folder(dir: &Path, now: i64) -> Result<()> {
    let state = dir.join(store::JSON_STATE);
    let moving = dir.join(store::JSON_MOVING);
    let moved = dir.join(format!("{}.moved", store::JSON_STATE));
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (has_state, has_moving) = (state.exists(), moving.exists());
    if !has_state && !has_moving {
        return upgrade_hook_only(dir);
    }
    if has_state && has_moving {
        fail!(
            "{} holds both {} and {}; one of them is left from a move that did not finish, and neither is deleted for you",
            dir.display(),
            store::JSON_STATE,
            store::JSON_MOVING
        );
    }
    let rows = lists_rows(dir)?;
    let _lock = take_lock(&dir.join("lists.lock"))?;
    if has_state {
        if let Some(version) = rows {
            fail!(
                "{} holds {} beside lists rows in {DATABASE} (version {version}); it can only come from a hand edit, and neither is deleted for you",
                dir.display(),
                store::JSON_STATE
            );
        }
        let lists = read_json(&state)?;
        // Everything the database refuses or the checks find is found here, before anything changes.
        rehearse(&lists, &name)?;
        std::fs::rename(&state, &moving)
            .context(|| format!("renaming {} to {}", state.display(), moving.display()))?;
    }
    let lists = read_json(&moving)?;
    match rows {
        None => {
            put_in_database(dir, &lists, &name, now)?;
        }
        Some(version) => {
            let last = {
                let path = dir.join(DATABASE);
                let conn = db::open_reader(&path)?.expect("rows were read");
                store::read_log(&conn)?.pop()
            };
            let ours = lists.1.version + 1 == version
                && last.is_some_and(|(v, _, by, words)| {
                    v == version && by == MOVE_BY && words == MOVE_WORDS
                });
            if !ours {
                fail!(
                    "{} holds {} at version {}, and {DATABASE} holds lists at version {version} that are not this move's; neither is deleted for you",
                    dir.display(),
                    store::JSON_MOVING,
                    lists.1.version
                );
            }
        }
    }
    std::fs::rename(&moving, &moved)
        .context(|| format!("renaming {} to {}", moving.display(), moved.display()))?;
    // Removed while the lock is still held; an old writer blocked on it then finds no lists.json.
    let _ = std::fs::remove_file(dir.join("lists.lock"));
    crate::text::note(format!(
        "moved project {name}'s lists into {}",
        dir.join(DATABASE).display()
    ));
    Ok(())
}

/// A database the hook stats made, with only their tables: given the lists'
/// tables and the schema's stamp, so every later open takes it as a store.
fn upgrade_hook_only(dir: &Path) -> Result<()> {
    let path = dir.join(DATABASE);
    if !path.is_file() {
        return Ok(());
    }
    let mut conn = db::open_for_move(&path)?;
    if db::stamp_of(&conn, &path)? != db::HOOK_ONLY_STAMP {
        return Ok(());
    }
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .sql()?;
    bring_to_schema(&tx, &path)?;
    tx.commit().sql()?;
    crate::text::note(format!("gave {} the lists' tables", path.display()));
    Ok(())
}

/// Creates the lists' tables in <tx> where only the hook stats' (or none)
/// are, stamps it, and refuses unless what it holds is what a fresh database
/// has.
fn bring_to_schema(tx: &Connection, path: &Path) -> Result<()> {
    let found = db::stamp_of(tx, path)?;
    if found == 0 {
        tx.execute_batch(&db::schema()).sql()?;
    } else {
        tx.execute_batch(&db::lists_schema()).sql()?;
    }
    tx.pragma_update(None, "user_version", db::stamp()).sql()?;
    if db::schema_rows(tx)? != db::schema_rows(&db::memory())? {
        fail!(
            "{} holds a schema that is not the one this builds makes; it is left as it was",
            path.display()
        );
    }
    Ok(())
}

type Source = (Lists, Json);

/// What `lists.json` held outside `Lists`: the counter and the log.
struct Json {
    next_id: u32,
    log: Vec<LogLine>,
    version: u64,
}

/// `lists.json` read as today's shape.
fn read_json(path: &Path) -> Result<Source> {
    let text = std::fs::read_to_string(path).context(|| format!("reading {}", path.display()))?;
    let mut value: Value = serde_json::from_str(&text)
        .map_err(|error| Fail::Message(format!("{} is not valid JSON: {error}", path.display())))?;
    let Some(object) = value.as_object_mut() else {
        fail!("{} is not an object", path.display())
    };
    let Some(next_id) = object.remove("next_id").and_then(|v| v.as_u64()) else {
        fail!("{} has no next_id", path.display())
    };
    let Some(log) = object.remove("log") else {
        fail!("{} has no log", path.display())
    };
    let log: Vec<LogLine> = serde_json::from_value(log).map_err(|error| {
        Fail::Message(format!(
            "{}'s log is not today's shape: {error}",
            path.display()
        ))
    })?;
    let lists: Lists = serde_json::from_value(value).map_err(|error| {
        Fail::Message(format!(
            "{} is not today's shape of the lists: {error}",
            path.display()
        ))
    })?;
    let version = lists.version;
    Ok((
        lists,
        Json {
            next_id: next_id as u32,
            log,
            version,
        },
    ))
}

/// What the database derives instead of storing must agree with what the JSON
/// says twice; each disagreement is named.
fn derived_disagreements(lists: &Lists, json: &Json) -> Vec<String> {
    use crate::lists::{How, Record};
    let mut faults = Vec::new();
    let mut ids: Vec<u32> = lists.questions.iter().map(|q| q.id.0).collect();
    ids.extend(lists.items.iter().map(|i| i.id.0));
    ids.extend(lists.designs.iter().map(|d| d.id.0));
    ids.extend(lists.decisions.iter().map(|d| d.id.0));
    ids.extend(lists.closed.iter().map(|c| c.record.id().0));
    if let Some(top) = ids.iter().max()
        && json.next_id <= *top
    {
        faults.push(format!(
            "next_id is {}, not above the highest id {top}",
            json.next_id
        ));
    }
    match json.log.last() {
        Some(last) if last.version != json.version => faults.push(format!(
            "version is {}, but the log ends at {}",
            json.version, last.version
        )),
        None if json.version != 0 => {
            faults.push(format!("version is {}, but the log is empty", json.version))
        }
        _ => {}
    }
    let all_items: Vec<&crate::lists::Item> = lists
        .items
        .iter()
        .chain(lists.closed.iter().filter_map(|c| match &c.record {
            Record::Item(item) => Some(item),
            _ => None,
        }))
        .collect();
    for decision in &lists.decisions {
        let answered_by_closing = lists.closed.iter().find_map(|c| match (&c.record, &c.how) {
            (Record::Question(q), How::Answered(d)) if *d == decision.id => Some(q.id),
            _ => None,
        });
        if answered_by_closing != decision.question {
            faults.push(format!(
                "decision {} says it answered {:?}, but the closings say {:?}",
                decision.id,
                decision.question.map(|q| q.0),
                answered_by_closing.map(|q| q.0)
            ));
        }
        let made: Vec<_> = all_items
            .iter()
            .filter(|i| i.from == Some(decision.id))
            .map(|i| i.id)
            .collect();
        let mut said = decision.work.clone();
        said.sort();
        let mut made_sorted = made.clone();
        made_sorted.sort();
        if said != made_sorted {
            faults.push(format!(
                "decision {} says it made {:?}, but the items' origins say {:?}",
                decision.id, said, made_sorted
            ));
        }
        if let Some(question) = decision.question {
            let asked = lists.closed.iter().find_map(|c| match &c.record {
                Record::Question(q) if q.id == question => Some(q.plan.clone()),
                _ => None,
            });
            if asked.is_some_and(|plan| plan != decision.plan) {
                faults.push(format!(
                    "decision {} has a plan other than its question {question}'s",
                    decision.id
                ));
            }
        }
    }
    for item in &all_items {
        if let Some(first) = item.running.first()
            && item
                .running
                .iter()
                .any(|r| r.brief != first.brief || r.since != first.since)
        {
            faults.push(format!(
                "item {} runs in places with different briefs or moments",
                item.id
            ));
        }
    }
    let landed_of = |id| {
        lists.closed.iter().find_map(|c| match (&c.record, &c.how) {
            (Record::Design(d), How::Landed(commits)) if d.id == id => {
                Some((commits.clone(), c.closed.at))
            }
            _ => None,
        })
    };
    let designs = lists
        .designs
        .iter()
        .map(|d| (d, None))
        .chain(lists.closed.iter().filter_map(|c| match &c.record {
            Record::Design(d) => Some((d, landed_of(d.id))),
            _ => None,
        }));
    for (design, closing) in designs {
        let said: Vec<(Vec<String>, i64)> = design
            .landed
            .iter()
            .map(|l| (l.commits.clone(), l.at))
            .collect();
        let derived: Vec<(Vec<String>, i64)> = closing.into_iter().collect();
        if said != derived {
            faults.push(format!(
                "design change {} says it landed as {:?}, but its closing says {:?}",
                design.id, said, derived
            ));
        }
    }
    faults
}

/// Builds the whole store in memory from <source> and checks it, naming every
/// fault; nothing on disk is touched.
fn rehearse(source: &Source, project: &str) -> Result<()> {
    let (lists, json) = source;
    let faults = derived_disagreements(lists, json);
    if !faults.is_empty() {
        fail!(
            "the lists of {project} cannot move into {DATABASE}:\n  {}",
            faults.join("\n  ")
        );
    }
    let mut conn = db::memory();
    let tx = conn.transaction().sql()?;
    move_in(&tx, source, project, 0)?;
    tx.rollback().sql()?;
    Ok(())
}

/// The database of <dir>, brought to the schema, with every row of <source>
/// in one transaction.
fn put_in_database(dir: &Path, source: &Source, project: &str, now: i64) -> Result<()> {
    let path = dir.join(DATABASE);
    let mut conn = db::open_for_move(&path)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .sql()?;
    let found = db::stamp_of(&tx, &path)?;
    if found == db::stamp() {
        let rows: i64 = tx
            .query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
            .sql()?;
        if rows > 0 {
            fail!("{} already holds lists", path.display());
        }
    } else {
        bring_to_schema(&tx, &path)?;
    }
    let left_out = move_in(&tx, source, project, now)?;
    tx.commit().sql()?;
    if left_out > 0 {
        crate::text::note(format!(
            "left out the {left_out} running entries of closed items: a closed item runs nowhere"
        ));
    }
    Ok(())
}

/// Puts every row of <source> into <db>, parents first, runs the code check
/// and returns how many running entries of closed items were left out. The
/// log is the file's, and the move's own line after it.
fn move_in(db: &Connection, source: &Source, project: &str, now: i64) -> Result<usize> {
    use crate::lists::item::Origin;
    use crate::lists::project::NewProject;
    use crate::lists::record::{move_in_closing, move_in_record};
    use crate::lists::{Kind, Record};
    let (lists, json) = source;
    lists::project::init(
        db,
        &NewProject {
            name: lists.project.clone(),
            repos: lists.repos.clone(),
            places: lists.design_places.clone(),
            sources: lists.design_sources.clone(),
            specs: lists.spec_files.clone(),
        },
    )?;
    lists::area::move_in(db, &lists.areas)?;
    let closed_items: Vec<&crate::lists::Item> = lists
        .closed
        .iter()
        .filter_map(|c| match &c.record {
            Record::Item(item) => Some(item),
            _ => None,
        })
        .collect();
    let closed_questions = lists.closed.iter().filter_map(|c| match &c.record {
        Record::Question(q) => Some(q),
        _ => None,
    });
    let closed_designs: Vec<&crate::lists::DesignChange> = lists
        .closed
        .iter()
        .filter_map(|c| match &c.record {
            Record::Design(d) => Some(d),
            _ => None,
        })
        .collect();
    let questions: Vec<&crate::lists::Question> =
        lists.questions.iter().chain(closed_questions).collect();
    let items: Vec<&crate::lists::Item> = lists
        .items
        .iter()
        .chain(closed_items.iter().copied())
        .collect();
    let designs: Vec<&crate::lists::DesignChange> = lists
        .designs
        .iter()
        .chain(closed_designs.iter().copied())
        .collect();
    // The record rows of every kind first: everything else points at them.
    let mut kinds: Vec<(crate::lists::Id, Kind, crate::lists::Stamp)> = Vec::new();
    kinds.extend(
        questions
            .iter()
            .map(|q| (q.id, Kind::Question, q.opened.clone())),
    );
    kinds.extend(items.iter().map(|i| (i.id, Kind::Item, i.opened.clone())));
    kinds.extend(
        designs
            .iter()
            .map(|d| (d.id, Kind::Design, d.opened.clone())),
    );
    kinds.extend(
        lists
            .decisions
            .iter()
            .map(|d| (d.id, Kind::Decision, d.recorded.clone())),
    );
    kinds.sort_by_key(|(id, _, _)| *id);
    for (id, kind, opened) in &kinds {
        move_in_record(db, *id, *kind, opened)?;
    }
    for question in &questions {
        lists::question::move_in(db, question)?;
    }
    let mut decisions: Vec<&crate::lists::Decision> = lists.decisions.iter().collect();
    decisions.sort_by_key(|d| d.id);
    for decision in decisions {
        lists::decision::move_in(db, decision)?;
    }
    for design in &designs {
        lists::design::move_in(db, design)?;
    }
    for item in &items {
        let origin = match item.from {
            None => None,
            Some(from) => Some(match kinds.iter().find(|(id, _, _)| *id == from) {
                Some((_, Kind::Decision, _)) => Origin::Decision(from),
                Some((_, Kind::Design, _)) => Origin::Design(from),
                _ => fail!(
                    "item {} was made by {from}, which is no decision or design change",
                    item.id
                ),
            }),
        };
        lists::item::move_in(db, item, origin)?;
    }
    for item in &lists.items {
        lists::item::move_in_children(db, item, false)?;
    }
    for item in &closed_items {
        lists::item::move_in_children(db, item, true)?;
    }
    for design in &designs {
        lists::design::move_in_children(db, design)?;
    }
    for closed in &lists.closed {
        move_in_closing(
            db,
            closed.record.id(),
            closed.record.kind(),
            &closed.how,
            &closed.closed,
        )?;
    }
    for line in &json.log {
        store::move_in_log(db, line.version, line.at, &line.by, &line.words)?;
    }
    store::move_in_log(db, json.version + 1, now, MOVE_BY, MOVE_WORDS)?;
    let faults = crate::lists::check::faults(db, project)?;
    if !faults.is_empty() {
        fail!(
            "the lists of {project} fail their check after the move:\n  {}",
            faults.join("\n  ")
        );
    }
    Ok(closed_items.iter().map(|item| item.running.len()).sum())
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;

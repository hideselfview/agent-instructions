//! The store: one folder per project under `Places::lists`, holding
//! `builds.sqlite` (the lists' tables, and the hook stats'; see db.rs). Every
//! write is one SQLite transaction taken with `BEGIN IMMEDIATE`: the command's
//! own inserts, updates and deletes through the owners' functions, the write
//! log's row, and the code check, then COMMIT. A write that is refused at any
//! point rolls back, so a refused command changes nothing. A reader sees one
//! committed snapshot and takes no write lock.
//!
//! A folder that still holds `lists.json` or `lists.json.moving` has lists
//! that `builds install` has not moved yet; every command refuses it, naming
//! that.
//!
//! Nothing here reads or writes outside the project's folder, except the
//! listing of the parent folder that finds which project a repo is in.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, TransactionBehavior, params};

use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::db::{self, DATABASE, Make, Sql};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::args::Args;
use crate::lists::check::{faults, project_name_fault};
use crate::lists::project::NewProject;
use crate::lists::record::{self, Closed, Record};
use crate::lists::{Lists, Stamp, area, decision, design, item, project, question};
use crate::places::Places;

/// The lists as the JSON store kept them, which `builds install` moves.
pub const JSON_STATE: &str = "lists.json";
/// The JSON store mid-move.
pub const JSON_MOVING: &str = "lists.json.moving";

/// One project's folder.
#[derive(Clone, Debug)]
pub struct Store {
    pub project: String,
    pub dir: PathBuf,
}

impl Store {
    /// The store of <project>; the name must be one a folder can have.
    pub fn named(places: &Places, project: &str) -> Result<Store> {
        if let Some(fault) = project_name_fault(project) {
            fail!("{fault}");
        }
        Ok(Store { project: project.to_string(), dir: places.lists.join(project) })
    }

    /// The store a command runs against: `--project`, else the project the
    /// repo of <cwd> belongs to.
    pub fn resolve(places: &Places, args: &Args, cwd: &Path) -> Result<Store> {
        if let Some(project) = args.value("project")? {
            return Store::named(places, project);
        }
        let checkout = match Checkout::at(cwd)? {
            Some(checkout) => checkout,
            None => fail!("{} is in no git checkout; say which project with --project", cwd.display()),
        };
        match project_of(places, &checkout)? {
            Some(project) => Store::named(places, &project),
            None => fail!("repo {} is in no project; say which with --project", checkout.repo()),
        }
    }

    pub fn database(&self) -> PathBuf {
        self.dir.join(DATABASE)
    }

    /// Refuses a folder whose lists are still in JSON.
    fn require_moved(&self) -> Result<()> {
        if self.dir.join(JSON_MOVING).exists() {
            fail!(
                "the lists of project {} are mid-move (`{}` is in {}); `builds install` finishes the move",
                self.project,
                JSON_MOVING,
                self.dir.display()
            );
        }
        if self.dir.join(JSON_STATE).exists() {
            fail!(
                "the lists of project {} are still in {JSON_STATE} ({}); `builds install` moves them",
                self.project,
                self.dir.display()
            );
        }
        Ok(())
    }

    fn no_lists(&self) -> Fail {
        Fail::Message(format!(
            "there are no lists for project {} ({} holds no project); `builds lists init {}` makes them",
            self.project,
            self.database().display(),
            self.project
        ))
    }

    /// The database for reading: None when there is none, or none with a
    /// project in it.
    fn reader(&self) -> Result<Option<Connection>> {
        self.require_moved()?;
        let Some(conn) = db::open_reader(&self.database())? else { return Ok(None) };
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0)).sql()?;
        Ok((rows > 0).then_some(conn))
    }

    /// Refuses a database whose project is another's.
    fn verify(&self, db: &Connection) -> Result<()> {
        let held = project::name(db)?;
        if held != self.project {
            fail!("{} holds the lists of project {held}, not {}", self.database().display(), self.project);
        }
        Ok(())
    }

    /// The state, checked: the code check runs on every read, so a store
    /// that fails it fails every command naming each fault. Takes no lock.
    pub fn load(&self) -> Result<Lists> {
        let Some(conn) = self.reader()? else { return Err(self.no_lists()) };
        self.verify(&conn)?;
        conn.execute_batch("BEGIN").sql()?;
        let lists = read(&conn)?;
        let found = faults(&conn, &self.project)?;
        conn.execute_batch("END").sql()?;
        refuse_faults(&self.project, &found)?;
        Ok(lists)
    }

    /// The repos of the project, without reading the rest.
    pub fn repos(&self) -> Result<Vec<String>> {
        let Some(conn) = self.reader()? else { return Err(self.no_lists()) };
        self.verify(&conn)?;
        Ok(project::read(&conn)?.repos)
    }

    /// The version of the last write, or the reason the store can't be read:
    /// the cheap look the daemon takes to know whether to load again.
    pub fn revision(&self) -> Result<u64> {
        let Some(conn) = self.reader()? else { return Err(self.no_lists()) };
        self.verify(&conn)?;
        version(&conn)
    }

    /// Makes the store with <new> as its first state: the project, repos and
    /// globs, and the log's first line for <words>. Refused when the store
    /// exists.
    pub fn create(&self, new: &NewProject, stamp: &Stamp, words: &str) -> Result<()> {
        if new.name != self.project {
            fail!("the project is named {}, not {}", new.name, self.project);
        }
        if let Some(fault) = new.refusal() {
            fail!("{fault}");
        }
        self.require_moved()?;
        std::fs::create_dir_all(&self.dir).context(|| format!("creating {}", self.dir.display()))?;
        let mut conn = db::open(&self.database(), Make::Allowed)?;
        commit(&mut conn, &self.project, stamp, words, |db| project::init(db, new))
    }

    /// Applies <apply> in one transaction and commits it, after the code
    /// check; the log gains a line for <words> by <stamp>. When <apply> or
    /// the check refuses, nothing is written.
    pub fn write<T>(&self, stamp: &Stamp, words: &str, apply: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.require_moved()?;
        if !self.database().is_file() {
            return Err(self.no_lists());
        }
        let mut conn = db::open(&self.database(), Make::Never)?;
        let held: i64 = conn.query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0)).sql()?;
        if held == 0 {
            return Err(self.no_lists());
        }
        self.verify(&conn)?;
        commit(&mut conn, &self.project, stamp, words, apply)
    }
}

/// Runs <apply> in a write transaction of <conn>, logs it, checks the whole
/// state and commits.
fn commit<T>(
    conn: &mut Connection,
    project: &str,
    stamp: &Stamp,
    words: &str,
    apply: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).sql()?;
    let result = apply(&tx)?;
    log_write(&tx, stamp, words)?;
    refuse_faults(project, &faults(&tx, project)?)?;
    tx.commit().sql()?;
    Ok(result)
}

fn refuse_faults(project: &str, found: &[String]) -> Result<()> {
    if found.is_empty() {
        return Ok(());
    }
    fail!("the lists of {project} fail their check, and nothing repairs them:\n  {}", found.join("\n  "));
}

/// The write log's next line, one version above the last.
pub fn log_write(db: &Connection, stamp: &Stamp, words: &str) -> Result<()> {
    db.execute(
        "INSERT INTO write_log (version, at, by, words) SELECT COALESCE(MAX(version), 0) + 1, ?1, ?2, ?3 FROM write_log",
        params![stamp.at, stamp.by, words],
    )
    .sql()?;
    Ok(())
}

/// Puts a line in the log as it was written elsewhere.
pub fn move_in_log(db: &Connection, version: u64, at: i64, by: &str, words: &str) -> Result<()> {
    db.execute("INSERT INTO write_log (version, at, by, words) VALUES (?1, ?2, ?3, ?4)", params![version as i64, at, by, words]).sql()?;
    Ok(())
}

/// The tables store.rs owns.
#[cfg(test)]
const TABLES: [&str; 1] = ["write_log"];

/// Writes a row of the write log as given, the constraints still holding:
/// for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(db: &Connection, table: &str, columns: &[(&str, rusqlite::types::Value)]) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

/// The version of the last write: 0 before the first.
pub fn version(db: &Connection) -> Result<u64> {
    db.query_row("SELECT COALESCE(MAX(version), 0) FROM write_log", [], |row| row.get::<_, i64>(0)).map(|v| v as u64).sql()
}

/// Every line of the write log, oldest first.
pub fn read_log(db: &Connection) -> Result<Vec<(u64, i64, String, String)>> {
    let mut statement = db.prepare("SELECT version, at, by, words FROM write_log ORDER BY version").sql()?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?, row.get(2)?, row.get(3)?))).sql()?;
    rows.map(|row| row.sql()).collect()
}

/// The whole state as one snapshot of <db>, each owner reading its part.
pub fn read(db: &Connection) -> Result<Lists> {
    let project = project::read(db)?;
    let opened = record::read_opened(db)?;
    let closings = record::read_closings(db)?;
    let items = item::read(db, &opened)?;
    let questions = question::read(db, &opened)?;
    let designs = design::read(db, &opened, &closings)?;
    let decisions = decision::read(db, &opened, &closings, &items, &questions)?;
    let areas = area::areas(db)?;
    let mut lists = Lists {
        project: project.name,
        repos: project.repos,
        design_places: project.places,
        design_sources: project.sources,
        spec_files: project.specs,
        version: version(db)?,
        areas,
        questions: Vec::new(),
        items: Vec::new(),
        designs: Vec::new(),
        decisions,
        closed: Vec::new(),
    };
    let mut closed: Vec<Closed> = Vec::new();
    for question in questions {
        match closings.get(&question.id) {
            Some((how, stamp)) => closed.push(Closed { record: Record::Question(question), how: how.clone(), closed: stamp.clone() }),
            None => lists.questions.push(question),
        }
    }
    for item in items {
        match closings.get(&item.id) {
            Some((how, stamp)) => closed.push(Closed { record: Record::Item(item), how: how.clone(), closed: stamp.clone() }),
            None => lists.items.push(item),
        }
    }
    for design in designs {
        match closings.get(&design.id) {
            Some((how, stamp)) => closed.push(Closed { record: Record::Design(design), how: how.clone(), closed: stamp.clone() }),
            None => lists.designs.push(design),
        }
    }
    // Closed records by closing time, then id.
    closed.sort_by_key(|c| (c.closed.at, c.record.id()));
    lists.closed = closed;
    Ok(lists)
}

/// The stores that exist under `Places::lists`, by project name: a folder
/// whose database holds a project, or whose lists are still in JSON. A folder
/// with only hook stats is no store.
pub fn stores(places: &Places) -> Result<Vec<Store>> {
    let entries = match std::fs::read_dir(&places.lists) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", places.lists.display()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", places.lists.display()))?;
        let path = entry.path();
        let in_json = path.join(JSON_STATE).is_file() || path.join(JSON_MOVING).is_file();
        let in_database = match db::open_reader(&path.join(DATABASE)) {
            Ok(Some(conn)) => conn.query_row("SELECT COUNT(*) FROM project", [], |row| row.get::<_, i64>(0)).map(|n| n > 0).unwrap_or(true),
            Ok(None) => false,
            // A database this builds can't read is a store that fails, named when it is loaded.
            Err(_) => path.join(DATABASE).is_file(),
        };
        if in_json || in_database {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names.iter().map(|name| Store::named(places, name)).collect()
}

/// One project's store as read: its lists, or what is wrong with them.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub project: String,
    pub lists: std::result::Result<Lists, String>,
}

/// Every store, each loaded and checked, for what reads them all (the
/// daemon, `builds health`): a store that fails stays in the answer with its
/// fault, since nothing repairs it. Err when the folder can't be listed.
pub fn load_all(places: &Places) -> std::result::Result<Vec<Loaded>, String> {
    let stores = stores(places).map_err(Fail::text)?;
    Ok(stores
        .into_iter()
        .map(|store| Loaded { lists: store.load().map_err(Fail::text), project: store.project })
        .collect())
}

/// What SQLite's integrity checks find in each store's database, one line
/// each: a foreign key that points at nothing, and a row that breaks a NOT
/// NULL or CHECK (`quick_check` verifies both). Only a store edited outside
/// builds, with the foreign keys off, can have them. A store that can't be
/// opened here is the load's to name.
pub fn integrity(places: &Places) -> Vec<String> {
    let Ok(stores) = stores(places) else { return Vec::new() };
    let mut found = Vec::new();
    for store in stores {
        // Opened for writing, though nothing is written: SQLite's checks of NOT NULL and CHECK
        // constraints do not run on a read-only connection.
        if !store.database().is_file() {
            continue;
        }
        let Ok(conn) = db::open(&store.database(), Make::Never) else { continue };
        let what = |fault: String| format!("lists: project {}'s database fails its integrity check: {fault}", store.project);
        if let Ok(mut statement) = conn.prepare("PRAGMA foreign_key_check") {
            let rows = statement.query_map([], |row| {
                Ok(format!(
                    "a row of {} points at nothing in {}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(2)?
                ))
            });
            for fault in rows.into_iter().flatten().flatten() {
                found.push(what(fault));
            }
        }
        if let Ok(mut statement) = conn.prepare("PRAGMA quick_check") {
            let rows = statement.query_map([], |row| row.get::<_, String>(0));
            for fault in rows.into_iter().flatten().flatten().filter(|fault| fault != "ok") {
                found.push(what(fault));
            }
        }
    }
    found
}

/// The stores read again after <previous>: a store whose last write is the
/// version <previous> holds, or whose fault is the one it had, is not loaded
/// again, and the second answer says whether anything differs from
/// <previous>. A write to the database that is not a lists write (a hook
/// run) moves no version, so it reads nothing more.
pub fn reload(
    places: &Places,
    previous: &std::result::Result<Vec<Loaded>, String>,
) -> (std::result::Result<Vec<Loaded>, String>, bool) {
    let stores = match stores(places) {
        Ok(stores) => stores,
        Err(error) => {
            let now = Err(error.text());
            let changed = *previous != now;
            return (now, changed);
        }
    };
    let held: &[Loaded] = previous.as_deref().unwrap_or(&[]);
    let mut changed = previous.is_err() || held.len() != stores.len();
    let mut next = Vec::new();
    for store in stores {
        let before = held.iter().find(|loaded| loaded.project == store.project);
        let kept = before.filter(|before| match (&before.lists, store.revision()) {
            (Ok(lists), Ok(version)) => lists.version == version,
            (Err(fault), Err(now)) => *fault == now.text(),
            _ => false,
        });
        match kept {
            Some(kept) => next.push(kept.clone()),
            None => {
                let loaded = Loaded { lists: store.load().map_err(Fail::text), project: store.project };
                changed |= before != Some(&loaded);
                next.push(loaded);
            }
        }
    }
    (Ok(next), changed)
}

/// The projects whose repos include <repo>, in name order.
pub fn projects_holding(places: &Places, repo: &str) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for store in stores(places)? {
        if store.repos()?.iter().any(|held| held == repo) {
            found.push(store.project);
        }
    }
    Ok(found)
}

/// The project <checkout>'s repo belongs to, or None. A repo in two projects
/// is refused.
pub fn project_of(places: &Places, checkout: &Checkout) -> Result<Option<String>> {
    let repo = checkout.repo();
    let mut found = projects_holding(places, &repo)?;
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        _ => fail!("repo {repo} is in two projects ({}); say which with --project", found.join(", ")),
    }
}

/// Who writes: `--by` when given, else the claim of the worktree <cwd> is in.
/// Neither is refused.
pub fn resolve_by(args: &Args, cwd: &Path) -> Result<String> {
    if let Some(by) = args.value("by")? {
        if by.trim().is_empty() {
            fail!("--by is empty");
        }
        return Ok(by.to_string());
    }
    if let Some(checkout) = Checkout::at(cwd)?
        && let Some(claim) = Claim::read(&checkout.claim_file())?
        && !claim.who.is_empty()
    {
        return Ok(claim.who);
    }
    fail!("say who you are with --by (this directory is in no claimed worktree)")
}

impl Stamp {
    /// The stamp of a write by <args>' `--by` or the claim at <cwd>, at <now>.
    pub fn resolve(now: i64, args: &Args, cwd: &Path) -> Result<Stamp> {
        Ok(Stamp { at: now, by: resolve_by(args, cwd)? })
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

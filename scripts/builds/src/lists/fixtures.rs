//! Lists for tests: a small valid state and the pieces to add to it, all
//! made through the owners' own writes.

use crate::lists::area::add_area;
use crate::lists::design::{Board, NewDesign, Resolution, ReviewSection, open_change, save};
use crate::lists::store::Store;
use crate::lists::item::{ItemDraft, ItemEdit, add_item, add_wait};
use crate::lists::project::{self, NewProject};
use crate::lists::question::{NewQuestion, ask, edit};
use crate::lists::store::log_write;
use crate::lists::{Audience, Id, Lists, QuestionKind, Stamp, Wait};
use rusqlite::Connection;
use std::collections::BTreeMap;

/// Day 2026-11-13 around noon, UTC; tests compare days by `local_day`, so
/// they stay within a day of this.
pub const NOON: i64 = 1_794_571_200;

pub fn stamp(at: i64) -> Stamp {
    Stamp { at, by: "coordinator".to_string() }
}

/// A store's lists in a new in-memory database: project `demo` over repos
/// bae and coven, areas Alpha and Beta, and the write log's first line
/// (`lists init demo`, version 1). No records.
pub fn db() -> Connection {
    let db = crate::db::memory();
    project::init(
        &db,
        &NewProject {
            name: "demo".into(),
            repos: vec!["bae".into(), "coven".into()],
            places: BTreeMap::new(),
            sources: BTreeMap::new(),
            specs: BTreeMap::new(),
        },
    )
    .unwrap();
    add_area(&db, "Alpha", None).unwrap();
    add_area(&db, "Beta", None).unwrap();
    log_write(&db, &stamp(NOON), "lists init demo").unwrap();
    db
}

/// The state <db> holds.
pub fn read(db: &Connection) -> Lists {
    crate::lists::store::read(db).unwrap()
}

/// The id the next record gets.
pub fn next_id(db: &Connection) -> Id {
    Id(db.query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM record", [], |row| row.get(0)).unwrap())
}

/// An open coordinator item in Alpha, opened at `NOON + id`, titled with
/// five words. Its waits are taken as they are: they need not be open.
pub fn item(db: &Connection, waits: Vec<Wait>) -> Id {
    let id = next_id(db);
    item_opened(db, waits, stamp(NOON + i64::from(id.0)))
}

/// `item`, opened as <opened> says.
pub fn item_opened(db: &Connection, waits: Vec<Wait>, opened: Stamp) -> Id {
    let id = next_id(db);
    let draft = ItemDraft {
        title: format!("Do the work numbered {}", id.0),
        area: "Alpha".into(),
        details: "details".into(),
        r#for: Audience::Coordinator,
        waits: Vec::new(),
        brief: None,
        priority: None,
    };
    add_item(db, &draft, None, &opened).unwrap();
    for wait in waits {
        add_wait(db, id, wait).unwrap();
    }
    id
}

/// Sets whose work open item <id> is.
pub fn set_audience(db: &Connection, id: Id, audience: Audience) {
    ItemEdit { title: None, area: None, details: None, brief: None, r#for: Some(audience) }.apply(db, id).unwrap();
}

/// Moves open item <id> to area <area>.
pub fn set_area(db: &Connection, id: Id, area: &str) {
    ItemEdit { title: None, area: Some(area.into()), details: None, brief: None, r#for: None }.apply(db, id).unwrap();
}

/// An open question that blocks nothing, opened at `NOON + id`.
pub fn question(db: &Connection) -> Id {
    let id = next_id(db);
    let new = NewQuestion {
        title: format!("Which way for number {}", id.0),
        details: "details".into(),
        kind: QuestionKind::Question,
        blocks_nothing: Some("nothing waits".into()),
        placed: None,
        plan: None,
    };
    ask(db, new, &stamp(NOON + i64::from(id.0))).unwrap()
}

/// An open question of <kind> that blocks nothing, opened at `NOON + id`.
pub fn question_kind(db: &Connection, kind: QuestionKind) -> Id {
    let id = next_id(db);
    let new = NewQuestion {
        title: format!("Which way for number {}", id.0),
        details: "details".into(),
        kind,
        blocks_nothing: Some("nothing waits".into()),
        placed: None,
        plan: None,
    };
    ask(db, new, &stamp(NOON + i64::from(id.0))).unwrap()
}

/// Everything <store> holds, for comparing before and after a command: the
/// lists as read and the write log.
pub fn state_of(store: &Store) -> String {
    let db = crate::db::open_reader(&store.database()).unwrap().expect("a store");
    format!("{:?}\n{:?}", crate::lists::store::read(&db).unwrap(), crate::lists::store::read_log(&db).unwrap())
}

/// A decision, made by `add_decision`, that is to answer open question <question>
/// (the question's closing, which names it, is the caller's).
pub fn decision_answering(db: &Connection, question: Id) -> Id {
    let new = crate::lists::decision::NewDecision {
        title: "Chose a way".into(),
        by: "person".into(),
        body: "why".into(),
        question: Some(question),
        replaces: None,
        plan: None,
    };
    crate::lists::decision::add_decision(db, &new, &[], &stamp(NOON)).unwrap().decision
}

/// Clears the why of open question <id> for blocking nothing: it must have
/// an item waiting on it, or fail its check.
pub fn blocks_something(db: &Connection, id: Id) {
    edit(db, id, None, None, Some(None)).unwrap();
}

/// Sets the why of open question <id> for blocking nothing.
pub fn blocks_nothing_because(db: &Connection, id: Id, why: &str) {
    edit(db, id, None, None, Some(Some(why.to_string()))).unwrap();
}

pub fn design(db: &Connection) -> Id {
    let id = next_id(db);
    let new = NewDesign {
        title: format!("Design change {}", id.0),
        details: "details".into(),
        repo: "bae".into(),
        worktree: "/d/bae-worktrees/side-2".into(),
        no_app_place: Some("shared files only".into()),
    };
    open_change(db, new, &stamp(NOON)).unwrap()
}

/// A design change drawn in <worktree>: each of <boards> (a board path and
/// its canvas link) is a place of it, checked by hand, with that link, and
/// <review> is its review page.
pub fn design_drawn(db: &Connection, worktree: &str, boards: &[(&str, &str)], review: Vec<ReviewSection>) -> Id {
    let id = design(db);
    let mut change = read(db).design(id).unwrap().clone();
    change.worktree = worktree.to_string();
    for (path, link) in boards {
        change.places.insert(path.to_string(), Resolution::Matches("checked".into()));
        change.boards.insert(path.to_string(), Board { link: link.to_string() });
    }
    change.review = review;
    save(db, &change).unwrap();
    id
}

/// The store of project `demo` over <places>, made as `lists init` and
/// `area add` make one: areas Alpha and Beta, over repos <repos>.
pub fn make_store(places: &crate::places::Places, repos: &[&str]) -> Store {
    let store = Store::named(places, "demo").unwrap();
    let new = NewProject {
        name: "demo".into(),
        repos: repos.iter().map(|r| r.to_string()).collect(),
        places: BTreeMap::new(),
        sources: BTreeMap::new(),
        specs: BTreeMap::new(),
    };
    store.create(&new, &stamp(NOON), "lists init demo").unwrap();
    store.write(&stamp(NOON), "area add Alpha", |db| add_area(db, "Alpha", None)).unwrap();
    store.write(&stamp(NOON), "area add Beta", |db| add_area(db, "Beta", None)).unwrap();
    store
}

/// A commit id made of <c>, for tests that close records as landed.
pub fn sha(c: char) -> String {
    c.to_string().repeat(40)
}

/// Resolves place <path> of open design change <design> as <resolution>.
pub fn design_place(db: &Connection, design: Id, path: &str, resolution: Resolution) {
    let mut change = read(db).design(design).unwrap().clone();
    change.places.insert(path.to_string(), resolution);
    save(db, &change).unwrap();
}

pub fn draft(title: &str) -> ItemDraft {
    ItemDraft {
        title: title.to_string(),
        area: "Alpha".into(),
        details: "details".into(),
        r#for: Audience::Coordinator,
        waits: Vec::new(),
        brief: None,
        priority: None,
    }
}

pub type Row = Vec<(&'static str, rusqlite::types::Value)>;

/// <base> with the columns <over> given instead.
pub fn row(base: &[(&'static str, rusqlite::types::Value)], over: &[(&'static str, rusqlite::types::Value)]) -> Row {
    let mut row: Row = base.iter().filter(|(name, _)| !over.iter().any(|(o, _)| o == name)).cloned().collect();
    row.extend(over.iter().cloned());
    row
}

/// A SQL value from a Rust one.
pub fn v(value: impl rusqlite::ToSql) -> rusqlite::types::Value {
    match value.to_sql().expect("a value") {
        rusqlite::types::ToSqlOutput::Owned(value) => value,
        rusqlite::types::ToSqlOutput::Borrowed(value) => match value {
            rusqlite::types::ValueRef::Null => rusqlite::types::Value::Null,
            rusqlite::types::ValueRef::Integer(n) => rusqlite::types::Value::Integer(n),
            rusqlite::types::ValueRef::Real(n) => rusqlite::types::Value::Real(n),
            rusqlite::types::ValueRef::Text(t) => rusqlite::types::Value::Text(String::from_utf8_lossy(t).into_owned()),
            rusqlite::types::ValueRef::Blob(b) => rusqlite::types::Value::Blob(b.to_vec()),
        },
        other => panic!("not a plain value: {other:?}"),
    }
}

/// The refusals a test collects for the named constraints of one owner's SQL
/// file: each is a row planted with every constraint holding, which the
/// database must refuse naming the constraint (for a CHECK) or the kind of
/// constraint (for a key or a foreign key). `done` checks that every named
/// constraint of the file has had its row refused.
pub struct Refusals {
    sql: &'static str,
    covered: std::collections::BTreeSet<String>,
}

impl Refusals {
    pub fn new(sql: &'static str) -> Refusals {
        Refusals { sql, covered: Default::default() }
    }

    /// <result> is refused naming the CHECK <name>.
    pub fn check(&mut self, name: &str, result: rusqlite::Result<usize>) {
        self.check_as(name, name, result);
    }

    /// <result> is refused with <expected> in the message: the kind of the
    /// key or foreign key <name>, which SQLite does not name.
    pub fn check_as(&mut self, name: &str, expected: &str, result: rusqlite::Result<usize>) {
        let message = match result {
            Ok(_) => panic!("{name}: the row was accepted"),
            Err(error) => error.to_string(),
        };
        assert!(message.contains(expected), "{name}: refused as '{message}', not '{expected}'");
        self.covered.insert(name.to_string());
    }

    /// Foreign keys and keys written as column constraints have no name of
    /// their own in the SQL; <cases> are the refusals this test made of them.
    pub fn covered_without_a_name(&mut self, cases: &[&str]) {
        for case in cases {
            assert!(self.covered.remove(*case), "{case} was not refused");
        }
    }

    pub fn done(self) {
        let mut named = crate::db::constraint_names(self.sql);
        named.sort();
        assert_eq!(self.covered.into_iter().collect::<Vec<_>>(), named, "a named constraint has no refused row");
    }
}

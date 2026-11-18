//! The work queue's items: what an item is, and every write to one. An item
//! waits on records or conditions, may carry a priority, a brief and notes,
//! and is made by a draft, and may name the record it came from (trail.rs),
//! such as the decision or design change that made it. Where it runs
//! is `running`, the worktree it holds with no agent is `kept`, what
//! already landed of it is `landing`; `item_command` is
//! the commands around these operations.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::check::item_title_fault;
use crate::lists::kept_work::KeptWork;
use crate::lists::running::{Run, Span};
use crate::lists::{
    Id, Kind, LandedPart, Lists, Located, Running, Stamp, Standing, area, kept_work, landing,
    record, running,
};
use crate::lists::{tag, trail};

/// Whose work an item is. The coordinator dispatches `Coordinator` items;
/// `DesignSession` items are shown apart and never listed as dispatchable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Audience {
    Coordinator,
    DesignSession,
}

/// What an item waits on. A condition is met only when removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wait {
    Item(Id),
    Question(Id),
    Design(Id),
    Condition(String),
}

/// A wait on a record: the waits that closing an item, a question or a
/// design change would leave unmet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordWait {
    Item(Id),
    Question(Id),
    Design(Id),
}

impl RecordWait {
    pub fn wait(self) -> Wait {
        match self {
            RecordWait::Item(id) => Wait::Item(id),
            RecordWait::Question(id) => Wait::Question(id),
            RecordWait::Design(id) => Wait::Design(id),
        }
    }

    /// The wait as `builds item wait` takes it: `item:N`, `question:N` or
    /// `design:N`.
    fn arg(self) -> String {
        match self {
            RecordWait::Item(id) => format!("item:{}", id.0),
            RecordWait::Question(id) => format!("question:{}", id.0),
            RecordWait::Design(id) => format!("design:{}", id.0),
        }
    }

    fn id(self) -> Id {
        match self {
            RecordWait::Item(id) | RecordWait::Question(id) | RecordWait::Design(id) => id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    First,
    Last,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Priority {
    pub level: Level,
    pub why: String,
}

/// A progress line on an item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub at: i64,
    pub by: String,
    pub text: String,
}

/// An entry of the work queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: Id,
    /// Five to seven words.
    pub title: String,
    pub area: String,
    pub details: String,
    #[serde(rename = "for")]
    pub r#for: Audience,
    pub waits: Vec<Wait>,
    pub priority: Option<Priority>,
    /// The implementation brief: path, then `#section` when it names one.
    pub brief: Option<String>,
    /// Where an agent works on it: one worktree per repo the work spans, none
    /// when it is not running.
    pub running: Vec<Running>,
    /// Its runs that ended, oldest first: the run that goes on is `running`.
    pub runs: Vec<Run>,
    /// The spans of its runs during which the worktree's claim note said
    /// held or the item waited on the person: the ended ones oldest first,
    /// then the open ones of the run that goes on.
    pub spans: Vec<Span>,
    /// Where it holds a worktree with no agent while it waits in the queue:
    /// one worktree per repo the work spans, none when it is not kept. An
    /// item is never both running and kept.
    pub kept: Vec<KeptWork>,
    pub landed: Vec<LandedPart>,
    /// What it held when the person dropped it: one entry per worktree it ran
    /// or kept in. An open item has none.
    pub dropped: Vec<crate::lists::dropped::DroppedWork>,
    /// The checks of its briefs against the product spec, oldest first.
    pub spec_checks: Vec<crate::lists::spec_check::SpecChecked>,
    /// Newest last.
    pub notes: Vec<Note>,
    /// The record it came from (trail.rs).
    pub origin: Option<Id>,
    pub opened: Stamp,
    /// The audits or groups it came from (tag.rs), in the order of their text.
    pub tags: Vec<String>,
}

impl Item {
    /// The item as a refusal names it, `#N "title"`, followed by ` of project
    /// P` when <project> gives the item's project P.
    pub fn named(&self, project: Option<&str>) -> String {
        match project {
            Some(project) => format!("{} \"{}\" of project {project}", self.id, self.title),
            None => format!("{} \"{}\"", self.id, self.title),
        }
    }
}

/// An item as a request writes it. Every field but `brief` and `priority`
/// is required: a draft says what the item waits on, even when nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemDraft {
    /// Five to seven plain words.
    pub title: String,
    pub area: String,
    pub details: String,
    #[serde(rename = "for")]
    pub r#for: Audience,
    pub waits: Vec<Wait>,
    pub brief: Option<String>,
    pub priority: Option<Priority>,
    /// The audits or groups it comes from (tag.rs); none when left out.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// The fields of an item an edit may change; at least one is set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemEdit {
    pub title: Option<String>,
    pub area: Option<String>,
    pub details: Option<String>,
    pub brief: Option<String>,
    #[serde(rename = "for")]
    pub r#for: Option<Audience>,
}
fn require_area(db: &Connection, area: &str) -> Result<()> {
    let areas = area::areas(db)?;
    if areas.iter().any(|known| known == area) {
        return Ok(());
    }
    let known = if areas.is_empty() {
        "there are none yet".to_string()
    } else {
        format!("the areas are: {}", areas.join("; "))
    };
    fail!("area '{area}' does not exist ({known}); `builds area add` makes one")
}

fn require_details(details: &str) -> Result<()> {
    if details.trim().is_empty() {
        fail!("details are empty; say what the work is");
    }
    Ok(())
}

/// Is <wait> one an item can take on now: an open record of the kind it
/// names? Names what is wrong and what to do.
pub fn require_open_wait(db: &Connection, wait: &Wait) -> Result<()> {
    let (id, kind) = match wait {
        Wait::Item(id) => (*id, Kind::Item),
        Wait::Question(id) => (*id, Kind::Question),
        Wait::Design(id) => (*id, Kind::Design),
        Wait::Condition(text) if text.trim().is_empty() => {
            fail!("a wait on a condition needs the condition's text")
        }
        Wait::Condition(_) => return Ok(()),
    };
    match record::standing(db, id)? {
        None => fail!("a wait names {id}, which no record has"),
        Some((found, _)) if found != kind => {
            fail!(
                "a wait names {id} as a {}, but it is a {}",
                kind.name(),
                found.name()
            )
        }
        Some((_, Some(closed))) => fail!(
            "a wait names {id}, which is closed ({}); a wait on a closed record never holds anything",
            closed.label()
        ),
        Some(_) => Ok(()),
    }
}

/// Item <id> is open, or the refusal.
pub fn require_open_item(db: &Connection, id: Id) -> Result<()> {
    match record::standing(db, id)? {
        Some((Kind::Item, None)) => Ok(()),
        _ => fail!("{id} is not an open item"),
    }
}
/// Item <id>, open; names what it is otherwise.
pub fn open_item(lists: &Lists, id: Id) -> Result<&Item> {
    match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located {
            kind: Kind::Item,
            standing: Standing::Open,
            ..
        }) => Ok(lists.item(id).expect("located open")),
        Some(Located {
            kind: Kind::Item,
            standing: Standing::Closed(closed),
            ..
        }) => {
            fail!("{id} is closed ({})", closed.how.label())
        }
        Some(Located { kind, .. }) => fail!("{id} is a {}, not an item", kind.name()),
    }
}
impl ItemDraft {
    /// The draft's own rules, against the state it lands in: title words, a
    /// known area, details, and every wait on an open record.
    pub fn validate(&self, db: &Connection) -> Result<()> {
        if let Some(fault) = item_title_fault(&self.title) {
            fail!("{fault}");
        }
        require_area(db, &self.area)?;
        require_details(&self.details)?;
        for wait in &self.waits {
            require_open_wait(db, wait)?;
        }
        if self.brief.as_deref().is_some_and(|b| b.trim().is_empty()) {
            fail!("brief is empty");
        }
        if let Some(priority) = &self.priority
            && priority.why.trim().is_empty()
        {
            fail!("a priority needs its why");
        }
        if let Some(fault) = self.tags.iter().find_map(|tag| tag::tag_fault(tag)) {
            fail!("{fault}");
        }
        Ok(())
    }
}

/// Adds the item <draft> describes, which came from record <origin> when
/// given (such as the decision or design change that made it); returns its
/// id.
pub fn add_item(
    db: &Connection,
    draft: &ItemDraft,
    origin: Option<Id>,
    stamp: &Stamp,
) -> Result<Id> {
    draft.validate(db)?;
    let id = record::open(db, Kind::Item, stamp)?;
    if let Some(origin) = origin {
        trail::require_record(db, origin)?;
    }
    insert(db, id, draft, origin)?;
    for wait in &draft.waits {
        insert_wait(db, id, wait)?;
    }
    if let Some(priority) = &draft.priority {
        insert_priority(db, id, priority)?;
    }
    tag::add_tags(db, id, &draft.tags)?;
    Ok(id)
}

/// The item's own row, for the record <id> already opened.
fn insert(db: &Connection, id: Id, draft: &ItemDraft, origin: Option<Id>) -> Result<()> {
    db.execute(
        "INSERT INTO item (id, title, area, details, audience, brief, origin) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            draft.title,
            draft.area,
            draft.details,
            audience_word(draft.r#for),
            draft.brief,
            origin
        ],
    )
    .sql()?;
    Ok(())
}

fn audience_word(audience: Audience) -> &'static str {
    match audience {
        Audience::Coordinator => "coordinator",
        Audience::DesignSession => "design-session",
    }
}

/// The word for <level>, as `--priority` takes it.
pub fn level_word(level: Level) -> &'static str {
    match level {
        Level::First => "first",
        Level::Last => "last",
    }
}

fn insert_priority(db: &Connection, id: Id, priority: &Priority) -> Result<()> {
    db.execute(
        "INSERT INTO item_priority (item_id, level, why) VALUES (?1, ?2, ?3) ON CONFLICT (item_id) DO UPDATE SET level = ?2, why = ?3",
        params![id, level_word(priority.level), priority.why],
    )
    .sql()?;
    Ok(())
}

/// Writes the wait of item <id>, once. A wait on a question clears the
/// reason the question blocks nothing in the same write, because the
/// question now blocks the item.
fn insert_wait(db: &Connection, id: Id, wait: &Wait) -> Result<()> {
    match wait {
        Wait::Item(on) => db.execute(
            "INSERT OR IGNORE INTO item_wait_item (item_id, on_item) VALUES (?1, ?2)",
            params![id, on],
        ),
        Wait::Question(on) => {
            crate::lists::question::clear_blocks_nothing(db, *on)?;
            db.execute(
                "INSERT OR IGNORE INTO item_wait_question (item_id, on_question) VALUES (?1, ?2)",
                params![id, on],
            )
        }
        Wait::Design(on) => db.execute(
            "INSERT OR IGNORE INTO item_wait_design (item_id, on_design) VALUES (?1, ?2)",
            params![id, on],
        ),
        Wait::Condition(text) => db.execute(
            "INSERT OR IGNORE INTO item_wait_condition (item_id, condition) VALUES (?1, ?2)",
            params![id, text],
        ),
    }
    .sql()?;
    Ok(())
}

impl ItemEdit {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.area.is_none()
            && self.details.is_none()
            && self.brief.is_none()
            && self.r#for.is_none()
    }

    /// Changes item <id> as the edit says, by the draft's rules.
    pub fn apply(&self, db: &Connection, id: Id) -> Result<()> {
        if self.is_empty() {
            fail!("the edit of {id} changes nothing: it names no field");
        }
        if let Some(title) = &self.title
            && let Some(fault) = item_title_fault(title)
        {
            fail!("{fault}");
        }
        if let Some(area) = &self.area {
            require_area(db, area)?;
        }
        if let Some(details) = &self.details {
            require_details(details)?;
        }
        if self.brief.as_deref().is_some_and(|b| b.trim().is_empty()) {
            fail!("brief is empty");
        }
        require_open_item(db, id)?;
        db.execute(
            "UPDATE item SET title = COALESCE(?2, title), area = COALESCE(?3, area), details = COALESCE(?4, details), \
             brief = COALESCE(?5, brief), audience = COALESCE(?6, audience) WHERE id = ?1",
            params![id, self.title, self.area, self.details, self.brief, self.r#for.map(audience_word)],
        )
        .sql()?;
        Ok(())
    }
}

/// Writes record <origin> as item <id>'s origin, for `trail::set_origin`,
/// which checks it.
pub(super) fn write_origin(db: &Connection, id: Id, origin: Id) -> Result<()> {
    db.execute(
        "UPDATE item SET origin = ?2 WHERE id = ?1",
        params![id, origin],
    )
    .sql()?;
    Ok(())
}

/// Appends <note> to open item <id>.
pub fn add_note(db: &Connection, id: Id, note: Note) -> Result<()> {
    require_open_item(db, id)?;
    db.execute(
        "INSERT INTO item_note (item_id, at, by, text) VALUES (?1, ?2, ?3, ?4)",
        params![id, note.at, note.by, note.text],
    )
    .sql()?;
    Ok(())
}

/// Makes open item <id> wait on <wait>, once.
pub fn add_wait(db: &Connection, id: Id, wait: Wait) -> Result<()> {
    require_open_item(db, id)?;
    insert_wait(db, id, &wait)
}

/// Ends open item <id>'s wait on <wait>.
pub fn remove_wait(db: &Connection, id: Id, wait: &Wait) -> Result<()> {
    require_open_item(db, id)?;
    let removed = match wait {
        Wait::Item(on) => db.execute(
            "DELETE FROM item_wait_item WHERE item_id = ?1 AND on_item = ?2",
            params![id, on],
        ),
        Wait::Question(on) => db.execute(
            "DELETE FROM item_wait_question WHERE item_id = ?1 AND on_question = ?2",
            params![id, on],
        ),
        Wait::Design(on) => db.execute(
            "DELETE FROM item_wait_design WHERE item_id = ?1 AND on_design = ?2",
            params![id, on],
        ),
        Wait::Condition(text) => db.execute(
            "DELETE FROM item_wait_condition WHERE item_id = ?1 AND condition = ?2",
            params![id, text],
        ),
    }
    .sql()?;
    if removed == 0 {
        fail!("{id} does not wait on that");
    }
    Ok(())
}

/// Turns every open item's wait on item <from> into a wait on question <to>;
/// returns the items that had one.
pub fn move_waits_to_question(db: &Connection, from: Id, to: Id) -> Result<Vec<Id>> {
    let mut statement = db
        .prepare(
            "SELECT item_id FROM item_wait_item WHERE on_item = ?1 \
             AND NOT EXISTS (SELECT 1 FROM closing WHERE record_id = item_id) ORDER BY item_id",
        )
        .sql()?;
    let moved: Vec<Id> = statement
        .query_map([from], |row| row.get(0))
        .sql()?
        .map(|row| row.sql())
        .collect::<Result<_>>()?;
    for id in &moved {
        db.execute(
            "DELETE FROM item_wait_item WHERE item_id = ?1 AND on_item = ?2",
            params![id, from],
        )
        .sql()?;
        insert_wait(db, *id, &Wait::Question(to))?;
    }
    Ok(moved)
}

/// Sets, or with None clears, open item <id>'s priority.
pub fn set_priority(db: &Connection, id: Id, priority: Option<Priority>) -> Result<()> {
    require_open_item(db, id)?;
    match priority {
        Some(priority) => insert_priority(db, id, &priority),
        None => {
            db.execute("DELETE FROM item_priority WHERE item_id = ?1", [id])
                .sql()?;
            Ok(())
        }
    }
}

/// The ids of the items in area <area>, open and closed, in id order.
pub fn items_in_area(db: &Connection, area: &str) -> Result<Vec<Id>> {
    let mut statement = db
        .prepare("SELECT id FROM item WHERE area = ?1 ORDER BY id")
        .sql()?;
    let ids = statement.query_map([area], |row| row.get(0)).sql()?;
    ids.map(|id| id.sql()).collect()
}

/// Every item, open and closed, as the rows hold it. <opened> is every
/// record's stamp.
pub fn read(
    db: &Connection,
    opened: &BTreeMap<Id, Stamp>,
    tags: &BTreeMap<Id, Vec<String>>,
) -> Result<Vec<Item>> {
    let mut waits: BTreeMap<Id, Vec<Wait>> = BTreeMap::new();
    for (table, column, make) in [
        ("item_wait_item", "on_item", Wait::Item as fn(Id) -> Wait),
        ("item_wait_question", "on_question", Wait::Question),
        ("item_wait_design", "on_design", Wait::Design),
    ] {
        let mut statement = db
            .prepare(&format!(
                "SELECT item_id, {column} FROM {table} ORDER BY {column}, item_id"
            ))
            .sql()?;
        for row in statement
            .query_map([], |row| Ok((row.get::<_, Id>(0)?, row.get::<_, Id>(1)?)))
            .sql()?
        {
            let (item, on) = row.sql()?;
            waits.entry(item).or_default().push(make(on));
        }
    }
    // Waits on records by the record's id, then conditions by text.
    for list in waits.values_mut() {
        list.sort_by_key(|wait| match wait {
            Wait::Item(id) | Wait::Question(id) | Wait::Design(id) => id.0,
            Wait::Condition(_) => u32::MAX,
        });
    }
    let mut statement = db
        .prepare("SELECT item_id, condition FROM item_wait_condition ORDER BY condition, item_id")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?
    {
        let (item, text) = row.sql()?;
        waits.entry(item).or_default().push(Wait::Condition(text));
    }
    let mut priorities: BTreeMap<Id, Priority> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT item_id, level, why FROM item_priority")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .sql()?
    {
        let (item, level, why) = row.sql()?;
        let level = if level == "first" {
            Level::First
        } else {
            Level::Last
        };
        priorities.insert(item, Priority { level, why });
    }
    let mut notes: BTreeMap<Id, Vec<Note>> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT item_id, at, by, text FROM item_note ORDER BY id")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                Note {
                    at: row.get(1)?,
                    by: row.get(2)?,
                    text: row.get(3)?,
                },
            ))
        })
        .sql()?
    {
        let (item, note) = row.sql()?;
        notes.entry(item).or_default().push(note);
    }
    let mut running = running::read(db)?;
    let mut runs = running::read_runs(db)?;
    let mut spans = running::read_spans(db)?;
    let mut kept = kept_work::read(db)?;
    let mut landed = landing::read(db)?;
    let mut dropped = crate::lists::dropped::read_all(db)?;
    let mut spec_checks = crate::lists::spec_check::read_all(db)?;
    let mut statement = db
        .prepare("SELECT id, title, area, details, audience, brief, origin FROM item ORDER BY id")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<Id>>(6)?,
            ))
        })
        .sql()?;
    let mut items = Vec::new();
    for row in rows {
        let (id, title, area, details, audience, brief, origin) = row.sql()?;
        items.push(Item {
            id,
            title,
            area,
            details,
            r#for: if audience == "coordinator" {
                Audience::Coordinator
            } else {
                Audience::DesignSession
            },
            waits: waits.remove(&id).unwrap_or_default(),
            priority: priorities.remove(&id),
            brief,
            running: running.remove(&id).unwrap_or_default(),
            runs: runs.remove(&id).unwrap_or_default(),
            spans: spans.remove(&id).unwrap_or_default(),
            kept: kept.remove(&id).unwrap_or_default(),
            landed: landed.remove(&id).unwrap_or_default(),
            dropped: dropped.remove(&id).unwrap_or_default(),
            spec_checks: spec_checks.remove(&id).unwrap_or_default(),
            notes: notes.remove(&id).unwrap_or_default(),
            origin,
            opened: opened[&id].clone(),
            tags: tags.get(&id).cloned().unwrap_or_default(),
        });
    }
    Ok(items)
}

impl Lists {
    pub fn item(&self, id: Id) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The open items whose waits name <wait>.
    pub fn open_items_waiting_on<'a>(&'a self, wait: &Wait) -> impl Iterator<Item = &'a Item> {
        self.items
            .iter()
            .filter(move |item| item.waits.contains(wait))
    }

    /// Refuses while an open item waits on the record <on> names, which the
    /// caller is about to close: names each such item and the `builds item
    /// wait` command that removes its wait on the record, and tells the
    /// reader to <how> the record after that.
    pub fn require_none_waits_on(&self, on: RecordWait, how: &str) -> Result<()> {
        let wait = on.wait();
        let waiting: Vec<&Item> = self.open_items_waiting_on(&wait).collect();
        if waiting.is_empty() {
            return Ok(());
        }
        let named: Vec<String> = waiting
            .iter()
            .map(|i| format!("{} \"{}\"", i.id, i.title))
            .collect();
        let commands: Vec<String> = waiting
            .iter()
            .map(|i| format!("`builds item wait {} --off {}`", i.id.0, on.arg()))
            .collect();
        let record = on.id();
        fail!(
            "{} wait{} on {record}; remove the wait{} first ({}), then {how} {record}",
            named.join("; "),
            if waiting.len() == 1 { "s" } else { "" },
            if waiting.len() == 1 { "" } else { "s" },
            commands.join("; ")
        )
    }
}

/// Rebuilds a store's `item` table from the old schema, where an origin was
/// only a decision or a design change and `origin_kind` named which, to the
/// schema in item.sql, and copies the old rows into it: each item keeps the
/// decision or design change that made it as its origin. The statements come
/// from item.sql, so the table equals a fresh database's. migrate.rs runs this
/// with the foreign keys off, and SQLite renames the old table under
/// `legacy_alter_table`, so the tables that point at `item` keep naming it.
pub fn move_to_origins(db: &Connection) -> Result<()> {
    const SQL: &str = include_str!("item.sql");
    crate::db::remake_table(
        db,
        "item",
        crate::db::statement(SQL, "CREATE TABLE item (", "STRICT;"),
        |db| {
            db.execute_batch(
                "INSERT INTO item (id, record_kind, title, area, details, audience, brief, origin)
    SELECT id, record_kind, title, area, details, audience, brief, origin FROM item_old;",
            )
            .sql()
        },
        &[
            crate::db::statement(SQL, "CREATE INDEX item_origin", ";"),
            crate::db::statement(SQL, "CREATE INDEX item_area", ";"),
        ],
    )
}

/// The tables item.rs owns.
#[cfg(test)]
const TABLES: [&str; 7] = [
    "item",
    "item_priority",
    "item_wait_item",
    "item_wait_question",
    "item_wait_design",
    "item_wait_condition",
    "item_note",
];

/// Writes a row of one of item.rs's tables as given, the constraints still
/// holding: for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "item_tests.rs"]
mod tests;

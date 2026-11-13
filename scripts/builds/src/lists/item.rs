//! The work queue's items: what an item is, and every write to one. An item
//! waits on records or conditions, may carry a priority, a brief and notes,
//! and is made by a draft or by a decision or design change. Where it runs
//! is `running`, what already landed of it is `landing`; `item_command` is
//! the commands around these operations.

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::Result;
use crate::lists::check::item_title_fault;
use crate::lists::{Id, Kind, LandedPart, Lists, Located, Running, Stamp, Standing};

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
    pub landed: Vec<LandedPart>,
    /// Newest last.
    pub notes: Vec<Note>,
    /// The decision or design change that made it.
    pub from: Option<Id>,
    pub opened: Stamp,
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
fn require_area(lists: &Lists, area: &str) -> Result<()> {
    if lists.areas.iter().any(|known| known == area) {
        return Ok(());
    }
    let known = if lists.areas.is_empty() {
        "there are none yet".to_string()
    } else {
        format!("the areas are: {}", lists.areas.join("; "))
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
pub fn require_open_wait(lists: &Lists, wait: &Wait) -> Result<()> {
    let (id, kind) = match wait {
        Wait::Item(id) => (*id, Kind::Item),
        Wait::Question(id) => (*id, Kind::Question),
        Wait::Design(id) => (*id, Kind::Design),
        Wait::Condition(text) if text.trim().is_empty() => fail!("a wait on a condition needs the condition's text"),
        Wait::Condition(_) => return Ok(()),
    };
    match lists.locate(id) {
        None => fail!("a wait names {id}, which no record has"),
        Some(Located { kind: found, .. }) if found != kind => {
            fail!("a wait names {id} as a {}, but it is a {}", kind.name(), found.name())
        }
        Some(Located { standing: Standing::Closed(closed), .. }) => fail!(
            "a wait names {id}, which is closed ({}); a wait on a closed record never holds anything",
            closed.how.label()
        ),
        Some(_) => Ok(()),
    }
}
/// Item <id>, open; names what it is otherwise.
pub fn open_item(lists: &Lists, id: Id) -> Result<&Item> {
    match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located { kind: Kind::Item, standing: Standing::Open, .. }) => Ok(lists.item(id).expect("located open")),
        Some(Located { kind: Kind::Item, standing: Standing::Closed(closed), .. }) => {
            fail!("{id} is closed ({})", closed.how.label())
        }
        Some(Located { kind, .. }) => fail!("{id} is a {}, not an item", kind.name()),
    }
}
impl ItemDraft {
    /// The draft's own rules, against the state it lands in: title words, a
    /// known area, details, and every wait on an open record.
    pub fn validate(&self, lists: &Lists) -> Result<()> {
        if let Some(fault) = item_title_fault(&self.title) {
            fail!("{fault}");
        }
        require_area(lists, &self.area)?;
        require_details(&self.details)?;
        for wait in &self.waits {
            require_open_wait(lists, wait)?;
        }
        if self.brief.as_deref().is_some_and(|b| b.trim().is_empty()) {
            fail!("brief is empty");
        }
        if let Some(priority) = &self.priority
            && priority.why.trim().is_empty()
        {
            fail!("a priority needs its why");
        }
        Ok(())
    }
}

/// Adds the item <draft> describes, made by <from> (a decision or a design
/// change) when it was; returns its id.
pub fn add_item(lists: &mut Lists, draft: &ItemDraft, from: Option<Id>, stamp: &Stamp) -> Result<Id> {
    draft.validate(lists)?;
    let id = lists.alloc_id();
    lists.items.push(Item {
        id,
        title: draft.title.clone(),
        area: draft.area.clone(),
        details: draft.details.clone(),
        r#for: draft.r#for,
        waits: draft.waits.clone(),
        priority: draft.priority.clone(),
        brief: draft.brief.clone(),
        running: Vec::new(),
        landed: Vec::new(),
        notes: Vec::new(),
        from,
        opened: stamp.clone(),
    });
    Ok(id)
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
    pub fn apply(&self, lists: &mut Lists, id: Id) -> Result<()> {
        if self.is_empty() {
            fail!("the edit of {id} changes nothing: it names no field");
        }
        if let Some(title) = &self.title
            && let Some(fault) = item_title_fault(title)
        {
            fail!("{fault}");
        }
        if let Some(area) = &self.area {
            require_area(lists, area)?;
        }
        if let Some(details) = &self.details {
            require_details(details)?;
        }
        if self.brief.as_deref().is_some_and(|b| b.trim().is_empty()) {
            fail!("brief is empty");
        }
        let Some(item) = lists.item_mut(id) else { fail!("{id} is not an open item") };
        if let Some(title) = &self.title {
            item.title = title.clone();
        }
        if let Some(area) = &self.area {
            item.area = area.clone();
        }
        if let Some(details) = &self.details {
            item.details = details.clone();
        }
        if let Some(brief) = &self.brief {
            item.brief = Some(brief.clone());
        }
        if let Some(audience) = self.r#for {
            item.r#for = audience;
        }
        Ok(())
    }
}

/// Appends <note> to open item <id>.
pub fn add_note(lists: &mut Lists, id: Id, note: Note) -> Result<()> {
    let Some(item) = lists.item_mut(id) else { fail!("{id} is not an open item") };
    item.notes.push(note);
    Ok(())
}

/// Makes open item <id> wait on <wait>, once.
pub fn add_wait(lists: &mut Lists, id: Id, wait: Wait) -> Result<()> {
    let Some(item) = lists.item_mut(id) else { fail!("{id} is not an open item") };
    if !item.waits.contains(&wait) {
        item.waits.push(wait);
    }
    Ok(())
}

/// Ends open item <id>'s wait on <wait>.
pub fn remove_wait(lists: &mut Lists, id: Id, wait: &Wait) -> Result<()> {
    let Some(item) = lists.item_mut(id) else { fail!("{id} is not an open item") };
    let Some(at) = item.waits.iter().position(|w| w == wait) else { fail!("{id} does not wait on that") };
    item.waits.remove(at);
    Ok(())
}

/// Turns every open item's wait on <from> into a wait on <to>; returns the
/// items that had one.
pub fn move_waits(lists: &mut Lists, from: &Wait, to: &Wait) -> Vec<Id> {
    let mut moved = Vec::new();
    for item in lists.items.iter_mut() {
        for wait in item.waits.iter_mut().filter(|w| *w == from) {
            *wait = to.clone();
            moved.push(item.id);
        }
    }
    moved
}

/// Sets, or with None clears, open item <id>'s priority.
pub fn set_priority(lists: &mut Lists, id: Id, priority: Option<Priority>) -> Result<()> {
    let Some(item) = lists.item_mut(id) else { fail!("{id} is not an open item") };
    item.priority = priority;
    Ok(())
}

/// Renames area <old> to <new> on every open item in it.
pub fn rename_area(lists: &mut Lists, old: &str, new: &str) {
    for item in lists.items.iter_mut().filter(|item| item.area == old) {
        item.area = new.to_string();
    }
}

impl Lists {
    pub fn item(&self, id: Id) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: Id) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// The open items whose waits name <wait>.
    pub fn open_items_waiting_on<'a>(&'a self, wait: &'a Wait) -> impl Iterator<Item = &'a Item> + 'a {
        self.items.iter().filter(move |item| item.waits.contains(wait))
    }
}

#[cfg(test)]
#[path = "item_tests.rs"]
mod tests;

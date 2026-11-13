//! What every record shares: the id space (one counter for questions, items,
//! design changes and decisions), the stamp of who and when, what a record
//! is, where it is (open or closed), and how it leaves its list. Closing a
//! record moves it from its own list to `closed`; the owners' own fields
//! stay as they were.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::Result;
use crate::lists::{DesignChange, Item, Lists, Question};

/// A record's id, from one counter per project; printed `#N`. Commands take
/// it as a bare number, since an unquoted `#12` is a comment in the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub u32);

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl Id {
    /// A bare number: `12`. `#12` is refused with the reason, since a shell
    /// never passes it on.
    pub fn parse(text: &str) -> Result<Id> {
        if let Some(bare) = text.strip_prefix('#') {
            fail!("ids are bare numbers, '{bare}', not '{text}' (an unquoted # starts a shell comment)");
        }
        match text.parse::<u32>() {
            Ok(n) if n >= 1 && text.bytes().all(|b| b.is_ascii_digit()) => Ok(Id(n)),
            _ => fail!("'{text}' is not an id: ids are whole numbers from 1"),
        }
    }

    /// `3,5,8`.
    pub fn parse_list(text: &str) -> Result<Vec<Id>> {
        text.split(',').map(Id::parse).collect()
    }
}

/// When and by whom: a write's moment (seconds since 1970) and the one who
/// wrote (`--by`, else the claim of the worktree the command ran in).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub at: i64,
    pub by: String,
}

/// How a record left its list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum How {
    Answered(Id),
    /// Settled from this rule or code, not by the person.
    Settled(String),
    Done,
    Landed(Vec<String>),
    Dropped(String),
    /// The open GitHub issue it moved to.
    Deferred(String),
    /// The question that took the item's place: the item was the person's.
    MovedToStack(Id),
}

/// A record that left its list, as it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Record {
    Question(Question),
    Item(Item),
    Design(DesignChange),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Closed {
    pub record: Record,
    pub how: How,
    pub closed: Stamp,
}

/// What a record is, open or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Question,
    Item,
    Design,
    Decision,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Question => "question",
            Kind::Item => "item",
            Kind::Design => "design change",
            Kind::Decision => "decision",
        }
    }
}

/// Where a record is.
#[derive(Clone, Copy, Debug)]
pub enum Standing<'a> {
    Open,
    Closed(&'a Closed),
}

#[derive(Clone, Copy, Debug)]
pub struct Located<'a> {
    pub kind: Kind,
    pub standing: Standing<'a>,
    pub title: &'a str,
}

impl How {
    /// The word flow counts it under.
    pub fn label(&self) -> &'static str {
        match self {
            How::Answered(_) => "answered",
            How::Settled(_) => "settled",
            How::Done => "done",
            How::Landed(_) => "landed",
            How::Dropped(_) => "dropped",
            How::Deferred(_) => "deferred",
            How::MovedToStack(_) => "moved-to-stack",
        }
    }

    /// Can a record of <kind> leave this way?
    pub fn fits(&self, kind: Kind) -> bool {
        match kind {
            Kind::Question => matches!(
                self,
                How::Answered(_) | How::Settled(_) | How::Done | How::Dropped(_) | How::Deferred(_)
            ),
            Kind::Item => matches!(
                self,
                How::Landed(_) | How::Dropped(_) | How::Deferred(_) | How::MovedToStack(_)
            ),
            Kind::Design => matches!(self, How::Landed(_)),
            Kind::Decision => false,
        }
    }
}

impl Record {
    pub fn id(&self) -> Id {
        match self {
            Record::Question(q) => q.id,
            Record::Item(i) => i.id,
            Record::Design(d) => d.id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Record::Question(q) => &q.title,
            Record::Item(i) => &i.title,
            Record::Design(d) => &d.title,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Record::Question(_) => Kind::Question,
            Record::Item(_) => Kind::Item,
            Record::Design(_) => Kind::Design,
        }
    }

    pub fn opened(&self) -> &Stamp {
        match self {
            Record::Question(q) => &q.opened,
            Record::Item(i) => &i.opened,
            Record::Design(d) => &d.opened,
        }
    }
}

impl Lists {
    /// Hands out the next id.
    pub fn alloc_id(&mut self) -> Id {
        let id = self.next_id;
        self.next_id = Id(id.0 + 1);
        id
    }

    pub fn closed_record(&self, id: Id) -> Option<&Closed> {
        self.closed.iter().find(|c| c.record.id() == id)
    }

    /// What <id> names and whether it is open or closed, or None when no
    /// record has it.
    pub fn locate(&self, id: Id) -> Option<Located<'_>> {
        if let Some(q) = self.question(id) {
            return Some(Located { kind: Kind::Question, standing: Standing::Open, title: &q.title });
        }
        if let Some(i) = self.item(id) {
            return Some(Located { kind: Kind::Item, standing: Standing::Open, title: &i.title });
        }
        if let Some(d) = self.design(id) {
            return Some(Located { kind: Kind::Design, standing: Standing::Open, title: &d.title });
        }
        if let Some(d) = self.decision(id) {
            return Some(Located { kind: Kind::Decision, standing: Standing::Open, title: &d.title });
        }
        let closed = self.closed_record(id)?;
        Some(Located { kind: closed.record.kind(), standing: Standing::Closed(closed), title: closed.record.title() })
    }
}

/// Closes open record <id> the way <how> says. Whether anything still waits
/// on it is the whole-state check's to refuse when the write ends.
pub fn close(lists: &mut Lists, id: Id, how: How, stamp: &Stamp) -> Result<()> {
    let kind = match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located { standing: Standing::Closed(closed), .. }) => {
            fail!("{id} is already closed ({})", closed.how.label())
        }
        Some(Located { kind, .. }) => kind,
    };
    if !how.fits(kind) {
        fail!("a {} cannot be closed as {}", kind.name(), how.label());
    }
    move_to_closed(lists, id, kind, how, stamp)
}

/// Takes open record <id>, a <kind>, out of its list into `closed`.
fn move_to_closed(lists: &mut Lists, id: Id, kind: Kind, how: How, stamp: &Stamp) -> Result<()> {
    let record = match kind {
        Kind::Question => {
            let at = lists.questions.iter().position(|q| q.id == id).expect("located open");
            Record::Question(lists.questions.remove(at))
        }
        Kind::Item => {
            let at = lists.items.iter().position(|i| i.id == id).expect("located open");
            Record::Item(lists.items.remove(at))
        }
        Kind::Design => {
            let at = lists.designs.iter().position(|d| d.id == id).expect("located open");
            Record::Design(lists.designs.remove(at))
        }
        Kind::Decision => fail!("decisions are never closed"),
    };
    lists.closed.push(Closed { record, how, closed: stamp.clone() });
    Ok(())
}

/// Sets the next id to hand out, whatever the ids already used.
#[cfg(test)]
pub fn plant_next_id(lists: &mut Lists, next: Id) {
    lists.next_id = next;
}

/// Closes open record <id> as <how> whether or not a record of its kind can
/// leave that way.
#[cfg(test)]
pub fn plant_closing(lists: &mut Lists, id: Id, how: How, stamp: &Stamp) {
    let kind = lists.locate(id).expect("a record").kind;
    move_to_closed(lists, id, kind, how, stamp).expect("an open record");
}

/// Changes how closed record <id> left, whether or not its kind can leave
/// that way.
#[cfg(test)]
pub fn plant_how(lists: &mut Lists, id: Id, how: How) {
    let closed = lists.closed.iter_mut().find(|c| c.record.id() == id).expect("a closed record");
    closed.how = how;
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;

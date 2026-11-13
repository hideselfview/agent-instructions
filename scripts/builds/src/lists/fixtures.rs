//! Lists for tests: a small valid state and the pieces to add to it, all
//! made through the owners' own writes.

use crate::lists::area::add_area;
use crate::lists::design::{NewDesign, open_change};
use crate::lists::item::{ItemDraft, add_item, add_wait};
use crate::lists::question::{NewQuestion, ask, edit};
use crate::lists::store::advance;
use crate::lists::{Audience, Id, Lists, QuestionKind, Stamp, Wait};
use std::collections::BTreeMap;

/// Day 2026-11-13 around noon, UTC; tests compare days by `local_day`, so
/// they stay within a day of this.
pub const NOON: i64 = 1_794_571_200;

pub fn stamp(at: i64) -> Stamp {
    Stamp { at, by: "coordinator".to_string() }
}

/// Project `demo` over repos bae and coven, areas Alpha and Beta, version 0
/// and no log: what `store.create` takes to make a store.
pub fn unwritten() -> Lists {
    let mut lists = Lists::empty("demo", vec!["bae".into(), "coven".into()], BTreeMap::new());
    add_area(&mut lists, "Alpha", None).unwrap();
    add_area(&mut lists, "Beta", None).unwrap();
    lists
}

/// `unwritten` after its first write; no records.
pub fn lists() -> Lists {
    let mut lists = unwritten();
    advance(&mut lists, &stamp(NOON), "lists init demo");
    lists
}

/// An open coordinator item in Alpha, opened at `NOON + id`, titled with
/// five words. Its waits are taken as they are: they need not be open.
pub fn item(lists: &mut Lists, waits: Vec<Wait>) -> Id {
    let id = lists.next_id;
    let draft = ItemDraft {
        title: format!("Do the work numbered {}", id.0),
        area: "Alpha".into(),
        details: "details".into(),
        r#for: Audience::Coordinator,
        waits: Vec::new(),
        brief: None,
        priority: None,
    };
    add_item(lists, &draft, None, &stamp(NOON + i64::from(id.0))).unwrap();
    for wait in waits {
        add_wait(lists, id, wait).unwrap();
    }
    id
}

/// An open question that blocks nothing, opened at `NOON + id`.
pub fn question(lists: &mut Lists) -> Id {
    let id = lists.next_id;
    let new = NewQuestion {
        title: format!("Which way for number {}", id.0),
        details: "details".into(),
        kind: QuestionKind::Question,
        blocks_nothing: Some("nothing waits".into()),
        placed: None,
        plan: None,
    };
    ask(lists, new, &stamp(NOON + i64::from(id.0)))
}

/// Clears the why of open question <id> for blocking nothing: it must have
/// an item waiting on it, or fail its check.
pub fn blocks_something(lists: &mut Lists, id: Id) {
    edit(lists, id, None, None, Some(None)).unwrap();
}

/// Sets the why of open question <id> for blocking nothing.
pub fn blocks_nothing_because(lists: &mut Lists, id: Id, why: &str) {
    edit(lists, id, None, None, Some(Some(why.to_string()))).unwrap();
}

pub fn design(lists: &mut Lists) -> Id {
    let id = lists.next_id;
    let new = NewDesign {
        title: format!("Design change {}", id.0),
        details: "details".into(),
        repo: "bae".into(),
        worktree: "/d/bae-worktrees/side-2".into(),
        no_app_place: Some("shared files only".into()),
    };
    open_change(lists, new, &stamp(NOON))
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

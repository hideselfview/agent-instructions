//! Lists for tests: a small valid state and the pieces to add to it.

use crate::lists::draft::ItemDraft;
use crate::lists::{
    Audience, DesignChange, Id, Item, Lists, LogLine, Question, QuestionKind, Stamp, Wait,
};
use std::collections::BTreeMap;

/// Day 2026-11-13 around noon, UTC; tests compare days by `local_day`, so
/// they stay within a day of this.
pub const NOON: i64 = 1_794_571_200;

pub fn stamp(at: i64) -> Stamp {
    Stamp { at, by: "coordinator".to_string() }
}

/// Project `demo` over repos bae and coven, areas Alpha and Beta, one log
/// line; no records.
pub fn lists() -> Lists {
    let mut lists = Lists::empty("demo", vec!["bae".into(), "coven".into()], BTreeMap::new());
    lists.areas = vec!["Alpha".into(), "Beta".into()];
    lists.version = 1;
    lists.log.push(LogLine { version: 1, at: NOON, by: "coordinator".into(), words: "lists init demo".into() });
    lists
}

/// An open coordinator item in Alpha, opened at `NOON + id`, titled with
/// five words.
pub fn item(lists: &mut Lists, waits: Vec<Wait>) -> Id {
    let id = lists.alloc_id();
    lists.items.push(Item {
        id,
        title: format!("Do the work numbered {}", id.0),
        area: "Alpha".into(),
        details: "details".into(),
        r#for: Audience::Coordinator,
        waits,
        priority: None,
        brief: None,
        running: Vec::new(),
        landed: Vec::new(),
        notes: Vec::new(),
        from: None,
        opened: stamp(NOON + i64::from(id.0)),
    });
    id
}

/// An open question that blocks nothing, opened at `NOON + id`.
pub fn question(lists: &mut Lists) -> Id {
    let id = lists.alloc_id();
    lists.questions.push(Question {
        id,
        title: format!("Which way for number {}", id.0),
        details: "details".into(),
        kind: QuestionKind::Question,
        blocks_nothing: Some("nothing waits".into()),
        placed: None,
        plan: None,
        opened: stamp(NOON + i64::from(id.0)),
    });
    id
}

pub fn design(lists: &mut Lists) -> Id {
    let id = lists.alloc_id();
    lists.designs.push(DesignChange {
        id,
        title: format!("Design change {}", id.0),
        details: "details".into(),
        repo: "bae".into(),
        worktree: "/d/bae-worktrees/side-2".into(),
        places: BTreeMap::new(),
        boards: BTreeMap::new(),
        review: Vec::new(),
        no_app_place: Some("shared files only".into()),
        approved: None,
        opened: stamp(NOON),
        landed: Vec::new(),
    });
    id
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

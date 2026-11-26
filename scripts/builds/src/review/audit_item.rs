//! The held item of an audit of main.
//!
//! An audit of main is a review landing under a key `audit:<name>`: it reads
//! code that is on main already, so no branch or agent waits on its findings,
//! and the person starts the fixes when they choose. Once no finding of an
//! audit waits for a settle, `hold` turns the confirmed ones into one item
//! that waits on a held entry of the stack, which shows under Waiting on you,
//! and ends the landing, so Needs attention never lists it.

use rusqlite::Connection;

use super::record::{self, Finding};
use crate::fail::Result;
use crate::lists::item::{ItemDraft, add_item};
use crate::lists::question::{NewQuestion, ask};
use crate::lists::{Audience, Effect, Id, QuestionKind, Stamp, Wait};

/// The area that holds the items of audits.
const AREA: &str = "audits";

/// The audit's name, for a landing key of an audit.
pub fn name_of(key: &str) -> Option<&str> {
    key.strip_prefix("audit:")
}

/// What `hold` made of an audit landing.
#[derive(Debug, PartialEq, Eq)]
pub enum Held {
    /// No finding of the audit stays confirmed, so the landing ended with
    /// nothing to fix.
    NothingToFix,
    /// The audit's confirmed findings became this item, held on this entry
    /// of the stack.
    Item { item: Id, entry: Id },
}

/// An open audit landing that no finding of waits on a settle: the one kind
/// `hold` takes.
pub struct Ready {
    landing: i64,
    name: String,
    repo: String,
    key: String,
    confirmed: Vec<Finding>,
}

impl Ready {
    /// The audit landing <landing> when it is open and no finding of it waits
    /// for a settle; None for any other landing.
    pub fn of(db: &Connection, landing: i64) -> Result<Option<Ready>> {
        let row = record::landing(db, landing)?;
        let Some(name) = name_of(&row.key) else {
            return Ok(None);
        };
        if !row.is_open() {
            return Ok(None);
        }
        let findings = record::findings_of_landing(db, landing)?;
        if findings.iter().any(Finding::is_unsettled) {
            return Ok(None);
        }
        Ok(Some(Ready {
            landing,
            name: name.to_string(),
            repo: row.repo.clone(),
            key: row.key.clone(),
            confirmed: findings.into_iter().filter(Finding::is_open).collect(),
        }))
    }
}

/// Makes the held item of the audit <ready>, and ends its landing as
/// abandoned with the item's number.
pub fn hold(db: &Connection, ready: &Ready, stamp: &Stamp) -> Result<Held> {
    let Ready {
        landing,
        name,
        repo,
        key,
        confirmed,
    } = ready;
    if confirmed.is_empty() {
        record::record_abandoned(
            db,
            *landing,
            "the audit has no confirmed finding left to fix",
            stamp.at,
        )?;
        return Ok(Held::NothingToFix);
    }
    if !crate::lists::area::areas(db)?
        .iter()
        .any(|known| known == AREA)
    {
        crate::lists::area::add_area(db, AREA, None)?;
    }
    let entry = ask(
        db,
        NewQuestion {
            title: format!("Start the fixes for audit {name}"),
            details: format!(
                "The audit {name} of {repo} has {} confirmed findings in code on main. The item that waits on this entry fixes them. Close this entry when you want the fixes to start.",
                confirmed.len()
            ),
            kind: QuestionKind::Held,
            effect: Effect::Cleanup,
            blocks_nothing: None,
            first: None,
            plan: None,
            origin: None,
        },
        stamp,
    )?;
    let item = add_item(
        db,
        &ItemDraft {
            title: format!("Fix the confirmed findings of audit {name}"),
            area: AREA.to_string(),
            details: details(repo, *landing, confirmed),
            r#for: Audience::Coordinator,
            waits: vec![Wait::Question(entry)],
            brief: None,
            priority: None,
            tags: vec![key.clone()],
        },
        Some(entry),
        stamp,
    )?;
    record::record_abandoned(
        db,
        *landing,
        &format!("its confirmed findings became item {item}"),
        stamp.at,
    )?;
    Ok(Held::Item { item, entry })
}

/// The item's details: where each finding is and what it says. Main may have
/// fixed some since the audit, so the agent checks each against main first.
fn details(repo: &str, landing: i64, confirmed: &[Finding]) -> String {
    let mut text = format!(
        "Fix the confirmed findings of review landing {landing} of {repo}, an audit of code on main. `builds review show {landing}` lists every finding with its settle. Main may have fixed some since the audit: check each finding against main, and fix the ones it still has.\n"
    );
    for finding in confirmed {
        text.push_str(&format!(
            "\nFinding {} ({}, {}): {}",
            finding.id, finding.rule, finding.file, finding.problem
        ));
    }
    text
}

#[cfg(test)]
#[path = "audit_item_tests.rs"]
mod tests;

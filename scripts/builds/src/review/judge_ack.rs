//! Tells the project's judge that it has not acknowledged the text every
//! agent reads. The judge idles until a message comes, and builds refuses
//! every command of an agent that has no acknowledgement, so a judge that
//! nobody told would find out only when it tries to settle. `builds health` calls `tell_outdated`.

use std::path::Path;

use crate::fail::Result;
use crate::inbox::{Sent, send};
use crate::lists::store::Store;
use crate::review::judge;

/// Sends the judge of <store>'s project, while its session runs
/// (`judge::listening`, the fact that `review settle` reads too), that it has
/// no acknowledgement and how to acknowledge. Returns the line to print, or
/// None when no judge listens or it has acknowledged the text.
pub fn tell_outdated(store: &Store, sessions: &Path) -> Result<Option<String>> {
    let conn = store.open_existing()?;
    let Some(judge) = judge::listening(&conn, sessions)? else {
        return Ok(None);
    };
    let Some(why) = crate::ack::outdated(store, &judge.session)? else {
        return Ok(None);
    };
    let project = &store.project;
    let message = format!(
        "You judge project {project}, and you have not acknowledged the text every agent reads: {why}. builds refuses your commands until you run `builds ack --agent {} --project {project}`, read every part and acknowledge it. Findings wait for you to settle.",
        judge.session
    );
    Ok(Some(match send(&judge.inbox, &message)? {
        Sent::Delivered => format!(
            "builds told the judge {} of project {project} to acknowledge the text: {why}",
            judge.by
        ),
        Sent::Gone => format!(
            "the inbox of the judge {} of project {project} is gone, so builds could not tell it to acknowledge the text: {why}",
            judge.by
        ),
    }))
}

#[cfg(test)]
#[path = "judge_ack_tests.rs"]
mod tests;

//! Tells the project's judge that its acknowledgement of the text every
//! agent reads is out of date. The judge idles until a message comes, and
//! builds refuses every command of an agent whose acknowledgement is out of
//! date, so a judge that nobody told would find out only when it tries to
//! settle. `builds health` and `builds install` call `tell_outdated`.

use std::path::Path;

use crate::fail::Result;
use crate::inbox::{Sent, send};
use crate::lists::store::Store;
use crate::review::judge;
use crate::tool::Tool;

/// Sends the judge of <store>'s project, while its session runs
/// (`judge::listening`, the fact that `review settle` reads too), what is
/// out of date in its acknowledgement and how to acknowledge again. Returns
/// the line to print, or None when no judge listens or its acknowledgement
/// holds.
pub fn tell_outdated(tool: &Tool, store: &Store, sessions: &Path) -> Result<Option<String>> {
    let conn = store.open_existing()?;
    let Some(judge) = judge::listening(&conn, sessions)? else {
        return Ok(None);
    };
    let Some(why) = crate::ack::outdated(&tool.places, store, &judge.session, tool.now)? else {
        return Ok(None);
    };
    let project = &store.project;
    let message = format!(
        "You judge project {project}, and your acknowledgement of the text every agent reads is out of date: {why}. builds refuses your commands until you run `builds ack --agent {} --project {project}`, read every part and acknowledge it. Findings wait for you to settle.",
        judge.session
    );
    Ok(Some(match send(&judge.inbox, &message)? {
        Sent::Delivered => format!(
            "builds told the judge {} of project {project} to acknowledge the text again: {why}",
            judge.by
        ),
        Sent::Gone => format!(
            "the inbox of the judge {} of project {project} is gone, so builds could not tell it to acknowledge the text again: {why}",
            judge.by
        ),
    }))
}

#[cfg(test)]
#[path = "judge_ack_tests.rs"]
mod tests;

//! Claims: who works in a side worktree, so another session doesn't take it.
//!
//! Whoever hands out the worktree (a coordinator dispatching an agent, a
//! person) claims it and releases it when the work lands or moves on. A claim
//! is not a lock: agents are a series of separate commands, so nothing could
//! hold one, and a session that ends without releasing leaves its claim behind
//! (status shows its age). It may carry a one-line note of where its work
//! stands, and the note may carry a state git can't show: waiting-on-you (a
//! question or step only the person can take) or winding-down (the agent's
//! last item, then its handoff). A claim whose who ends in "(landing)" is the
//! worktree its coordinator lands commits from.
//!
//! The file is one line in the worktree's git dir: `<who>\t<since>`, then
//! `\t<note>` when it has a note, then `\t<state>` when the note has a state.

use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::Result;
use crate::text::{local_minute, note};

/// The states a note may carry.
pub const NOTE_STATES: [&str; 2] = ["waiting-on-you", "winding-down"];

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Claim {
    pub who: String,
    pub since: String,
    pub note: Option<String>,
    pub note_state: Option<String>,
}

impl Claim {
    /// The claim in <file>, or None when there is none.
    pub fn read(file: &Path) -> Result<Option<Claim>> {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => fail!("reading {}: {error}", file.display()),
        };
        let line = text.lines().next().unwrap_or("");
        let mut fields = line.split('\t').filter(|field| !field.is_empty());
        let field = |value: Option<&str>| value.map(str::to_string);
        Ok(Some(Claim {
            who: fields.next().unwrap_or("").to_string(),
            since: fields.next().unwrap_or("").to_string(),
            note: field(fields.next()),
            note_state: field(fields.next()),
        }))
    }

    pub fn write(&self, file: &Path) -> Result<()> {
        let mut line = format!("{}\t{}", self.who, self.since);
        match (&self.note, &self.note_state) {
            (Some(text), Some(state)) => line += &format!("\t{text}\t{state}"),
            (Some(text), None) => line += &format!("\t{text}"),
            (None, Some(_)) => fail!("a note's state needs a note"),
            (None, None) => {}
        }
        line.push('\n');
        std::fs::write(file, line).map_err(|error| {
            fail::Fail::Message(format!("writing {}: {error}", file.display()))
        })
    }

    /// `<who> since <since>`.
    pub fn text(&self) -> String {
        format!("{} since {}", self.who, self.since)
    }
}

/// The claim's fields are tab-separated on one line, so none may hold either.
pub fn require_one_line(what: &str, value: &str) -> Result<()> {
    if value.contains(['\t', '\n', '\r']) {
        fail!("{what} must be one line without tabs");
    }
    Ok(())
}

/// Claims <top> for <who> from <now>, keeping the note and its state when
/// given.
pub fn write_claim(checkout: &Checkout, who: &str, now: i64, note: Option<String>, state: Option<String>) -> Result<()> {
    Claim { who: who.to_string(), since: local_minute(now), note, note_state: state }
        .write(&checkout.claim_file())
}

/// `builds claim <checkout> <who>`. Claiming it again for the one who holds
/// it renews the claim's date and keeps its note and the note's state.
pub fn cmd_claim(args: &[String], now: i64) -> Result<()> {
    let [checkout, who] = args else { fail!("usage: builds claim <checkout> <who>") };
    let checkout = Checkout::find(checkout)?;
    if who.is_empty() {
        fail!("usage: builds claim <checkout> <who>");
    }
    require_one_line("<who>", who)?;
    if checkout.is_main() {
        fail!("{} is the main checkout; only side worktrees are claimed", checkout.top.display());
    }
    let (kept_note, kept_state) = match Claim::read(&checkout.claim_file())? {
        Some(claim) if claim.who != *who => fail!(
            "{} is claimed by {}; builds release it first if that work is over",
            checkout.top.display(),
            claim.text()
        ),
        Some(claim) => (claim.note, claim.note_state),
        None => (None, None),
    };
    write_claim(&checkout, who, now, kept_note, kept_state)?;
    note(format!("claimed {} for {who}", checkout.top.display()));
    Ok(())
}

/// `builds note <checkout> [--state <state>] [<text>]`: sets the claim's
/// note to the rest of the arguments joined by spaces, with the state
/// --state names or none, or clears note and state when there is no text. The
/// claim's date stays.
pub fn cmd_note(args: &[String]) -> Result<()> {
    let usage = format!("usage: builds note <checkout> [--state {}] [<text>]", NOTE_STATES.join("|"));
    let Some((checkout, mut rest)) = args.split_first() else { fail!("{usage}") };
    let checkout = Checkout::find(checkout)?;
    let mut state = None;
    if rest.first().map(String::as_str) == Some("--state") {
        let named = rest.get(1).cloned().unwrap_or_default();
        if !NOTE_STATES.contains(&named.as_str()) {
            fail!("note: --state is one of {}, got '{named}'", NOTE_STATES.join(", "));
        }
        state = Some(named);
        rest = &rest[2.min(rest.len())..];
    }
    let text = rest.join(" ");
    require_one_line("a note", &text)?;
    if text.is_empty()
        && let Some(state) = &state {
            fail!("note: --state {state} needs the note's text, what it is about");
        }
    let file = checkout.claim_file();
    let Some(claim) = Claim::read(&file)? else {
        fail!(
            "{} has no claim; a note belongs to a claim (builds claim it first)",
            checkout.top.display()
        );
    };
    let top = checkout.top.display();
    let message = match (&state, text.is_empty()) {
        (Some(state), _) => format!("noted on {top}'s claim ({}): {text}", state.replace('-', " ")),
        (None, false) => format!("noted on {top}'s claim: {text}"),
        (None, true) => format!("cleared the note on {top}'s claim"),
    };
    Claim {
        note: if text.is_empty() { None } else { Some(text) },
        note_state: state,
        ..claim
    }
    .write(&file)?;
    note(message);
    Ok(())
}

/// `builds release <checkout>`.
pub fn cmd_release(args: &[String]) -> Result<()> {
    let [checkout] = args else { fail!("usage: builds release <checkout>") };
    let checkout = Checkout::find(checkout)?;
    let file = checkout.claim_file();
    let Some(claim) = Claim::read(&file)? else {
        note(format!("{} has no claim", checkout.top.display()));
        return Ok(());
    };
    note(format!("released {} (was {})", checkout.top.display(), claim.text()));
    std::fs::remove_file(&file)
        .map_err(|error| fail::Fail::Message(format!("removing {}: {error}", file.display())))
}

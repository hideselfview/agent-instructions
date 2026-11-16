//! Claims: who works in a side worktree, so another session doesn't take it.
//!
//! Whoever hands out the worktree (a coordinator dispatching an agent, a
//! person) claims it and releases it when the work lands or moves on. A claim
//! is not a lock: agents are a series of separate commands, so nothing could
//! hold one, and a session that ends without releasing leaves its claim behind
//! (status shows its age). It may carry a one-line note of where its work
//! stands, and the note may carry a state git can't show: waiting-on-you (a
//! question or step only the person can take), winding-down (the agent's
//! last item, then its handoff) or held (kept on purpose with no work
//! running, the note saying why and until when).
//!
//! The file is one line in the worktree's git dir: `<who>\t<since>`, then
//! `\t<note>` when it has a note, then `\t<state>` when the note has a state.

use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::Result;
use crate::places::Places;
use crate::text::{local_minute, local_minute_epoch, note};

/// The states a note may carry.
pub const NOTE_STATES: [&str; 3] = ["waiting-on-you", "winding-down", "held"];

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Claim {
    pub who: String,
    /// When the claim began, to the minute. In JSON it is the `YYYY-MM-DD HH:MM`
    /// text that the claim file and `builds status` show.
    #[serde(with = "minute_text")]
    pub since: i64,
    pub note: Option<String>,
    pub note_state: Option<String>,
}

/// A claim's date as the text of its file in JSON.
mod minute_text {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(epoch: &i64, out: S) -> Result<S::Ok, S::Error> {
        out.serialize_str(&crate::text::local_minute(*epoch))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<i64, D::Error> {
        let text = String::deserialize(from)?;
        crate::text::local_minute_epoch(&text)
            .ok_or_else(|| D::Error::custom(format!("{text:?} is not YYYY-MM-DD HH:MM")))
    }
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
        let who = fields.next().unwrap_or("").to_string();
        let date = fields.next().unwrap_or("");
        let Some(since) = local_minute_epoch(date) else {
            fail!(
                "{}: the claim's date is {date:?}, not YYYY-MM-DD HH:MM",
                file.display()
            );
        };
        Ok(Some(Claim {
            who,
            since,
            note: field(fields.next()),
            note_state: field(fields.next()),
        }))
    }

    pub fn write(&self, file: &Path) -> Result<()> {
        let mut line = format!("{}\t{}", self.who, local_minute(self.since));
        match (&self.note, &self.note_state) {
            (Some(text), Some(state)) => line += &format!("\t{text}\t{state}"),
            (Some(text), None) => line += &format!("\t{text}"),
            (None, Some(_)) => fail!("a note's state needs a note"),
            (None, None) => {}
        }
        line.push('\n');
        std::fs::write(file, line)
            .map_err(|error| fail::Fail::Message(format!("writing {}: {error}", file.display())))
    }

    /// `<who> since <since>`.
    pub fn text(&self) -> String {
        format!("{} since {}", self.who, local_minute(self.since))
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
pub fn write_claim(
    checkout: &Checkout,
    who: &str,
    now: i64,
    note: Option<String>,
    state: Option<String>,
) -> Result<()> {
    Claim {
        who: who.to_string(),
        since: now,
        note,
        note_state: state,
    }
    .write(&checkout.claim_file())
}

/// Records the project <checkout>'s work is in, or with None forgets it. The
/// project must hold the worktree's repo.
pub fn set_project(places: &Places, checkout: &Checkout, project: Option<&str>) -> Result<()> {
    let file = checkout.project_file();
    let Some(project) = project else {
        return remove_if_present(&file);
    };
    if let Some(fault) = crate::lists::check::project_name_fault(project) {
        fail!("{fault}");
    }
    let repo = checkout.repo();
    if !crate::lists::store::projects_holding(places, &repo)?
        .iter()
        .any(|held| held == project)
    {
        fail!("project {project} holds no repo {repo}; builds lists init or add-repo makes it so");
    }
    std::fs::write(&file, format!("{project}\n"))
        .map_err(|error| fail::Fail::Message(format!("writing {}: {error}", file.display())))
}

/// Refuses a claim that names no project (<named> is the project the claim
/// gives or the worktree keeps) of a worktree of <repo> when two projects hold
/// the repo. The lists commands, `builds release` and the dashboard find the
/// worktree's project from its claim, so without one the worktree's items
/// would not show and its claim could not be released.
pub fn require_project_named(places: &Places, repo: &str, named: Option<&str>) -> Result<()> {
    if named.is_some() {
        return Ok(());
    }
    let holding = crate::lists::store::projects_holding(places, repo)?;
    if holding.len() > 1 {
        fail!(
            "repo {repo} is in two projects ({}); name the project of the work with --project",
            holding.join(", ")
        );
    }
    Ok(())
}

/// `builds claim <checkout> <who> [--project P]`. Claiming it again for the
/// one who holds it renews the claim's date and keeps its note and the note's
/// state. --project names the project the work is in, for a repo that two
/// projects hold, and a worktree of such a repo must have one; without it the
/// worktree keeps the project it had.
pub fn cmd_claim(places: &Places, args: &[String], now: i64) -> Result<()> {
    let usage = "usage: builds claim <checkout> <who> [--project P]";
    let (positional, project) = match args {
        [checkout, who] => ([checkout, who], None),
        [checkout, who, flag, project] if flag == "--project" => ([checkout, who], Some(project)),
        _ => fail!("{usage}"),
    };
    let [checkout, who] = positional;
    let checkout = Checkout::find(checkout)?;
    if who.is_empty() {
        fail!("{usage}");
    }
    require_one_line("<who>", who)?;
    if !checkout.is_side() {
        let what = if checkout.is_main() {
            "the main checkout"
        } else {
            "not one of the repo's side worktrees"
        };
        fail!(
            "{} is {what}; only side worktrees are claimed",
            checkout.top.display()
        );
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
    match project {
        Some(project) => set_project(places, &checkout, Some(project))?,
        None => {
            let kept = checkout.claimed_project()?;
            require_project_named(places, &checkout.repo(), kept.as_deref())?;
        }
    }
    write_claim(&checkout, who, now, kept_note, kept_state)?;
    note(format!("claimed {} for {who}", checkout.top.display()));
    Ok(())
}

/// `builds note <checkout> [--state <state>] [<text>]`: sets the claim's
/// note to the rest of the arguments joined by spaces, with the state
/// --state names or none, or clears note and state when there is no text. The
/// claim's date stays. --state held is refused unless every item running in
/// the worktree waits on an open record (lists/held.rs).
pub fn cmd_note(places: &Places, args: &[String]) -> Result<()> {
    let usage = format!(
        "usage: builds note <checkout> [--state {}] [<text>]",
        NOTE_STATES.join("|")
    );
    let Some((checkout, mut rest)) = args.split_first() else {
        fail!("{usage}")
    };
    let checkout = Checkout::find(checkout)?;
    let mut state = None;
    if rest.first().map(String::as_str) == Some("--state") {
        let named = rest.get(1).cloned().unwrap_or_default();
        if !NOTE_STATES.contains(&named.as_str()) {
            fail!(
                "note: --state is one of {}, got '{named}'",
                NOTE_STATES.join(", ")
            );
        }
        state = Some(named);
        rest = &rest[2.min(rest.len())..];
    }
    let text = rest.join(" ");
    require_one_line("a note", &text)?;
    if text.is_empty()
        && let Some(state) = &state
    {
        fail!("note: --state {state} needs the note's text, what it is about");
    }
    let file = checkout.claim_file();
    let Some(claim) = Claim::read(&file)? else {
        fail!(
            "{} has no claim; a note belongs to a claim (builds claim it first)",
            checkout.top.display()
        );
    };
    if state.as_deref() == Some("held") {
        crate::lists::held::require_hold_allowed(places, &checkout)?;
    }
    let top = checkout.top.display();
    let message = match (&state, text.is_empty()) {
        (Some(state), _) => format!(
            "noted on {top}'s claim ({}): {text}",
            state.replace('-', " ")
        ),
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

/// `builds release <checkout>`, refused while an item of the lists runs in
/// the worktree, since the claim names the item's agent, and while the
/// worktree holds work origin/main lacks that no item holds
/// (`require_nothing_unlanded`).
pub fn cmd_release(places: &Places, args: &[String]) -> Result<()> {
    let [checkout] = args else {
        fail!("usage: builds release <checkout>")
    };
    let checkout = Checkout::find(checkout)?;
    crate::lists::running::require_release_allowed(places, &checkout)?;
    if let Some(claim) = Claim::read(&checkout.claim_file())? {
        let commits = crate::procs::git_commits(&crate::procs::Processes::read());
        let held = crate::unlanded::unheld_in(places, &checkout, claim.since, &commits)?;
        if !held.is_empty() {
            fail!(
                "{}",
                crate::unlanded::refusal("release", &checkout.top, &held)
            );
        }
    }
    match forget(&checkout)? {
        Some(claim) => note(format!(
            "released {} (was {})",
            checkout.top.display(),
            claim.text()
        )),
        None => note(format!("{} has no claim", checkout.top.display())),
    }
    Ok(())
}

/// Removes <checkout>'s claim and the project file it names, and returns the
/// claim, or None when there was none. A project file left behind would give
/// its project to a later claim that names no project. forget does not check
/// the worktree for work origin/main lacks; `cmd_release` and each other
/// caller check that first.
pub fn forget(checkout: &Checkout) -> Result<Option<Claim>> {
    let file = checkout.claim_file();
    let claim = Claim::read(&file)?;
    remove_if_present(&checkout.project_file())?;
    remove_if_present(&file)?;
    Ok(claim)
}

/// Removes <checkout>'s claim and keeps its project file, and returns the
/// claim, or None when there was none: an item parked there (lists/parked.rs)
/// keeps the worktree in its project for the agent that takes it next.
pub fn unclaim(checkout: &Checkout) -> Result<Option<Claim>> {
    let file = checkout.claim_file();
    let claim = Claim::read(&file)?;
    remove_if_present(&file)?;
    Ok(claim)
}

/// Removes the file at <path>; a file that is already gone is no error.
fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => fail!("removing {}: {error}", path.display()),
    }
}

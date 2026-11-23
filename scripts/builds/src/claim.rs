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
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::text::{local_minute, local_minute_epoch, note};
use crate::whole_file::remove_if_present;

/// The note of a claim in the starting state.
const STARTING_TEXT: &str = "claimed; its item is not yet taken";

/// The states a note set by `builds note` may carry.
pub const NOTE_STATES: [&str; 3] = ["waiting-on-you", "winding-down", "held"];

/// A claim's note: one line of where its work stands, and the state git can't
/// show. A state has a note, so a claim holds one `Option<Note>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// A note with no state.
    Plain(String),
    /// The work waits on a question or a step only the person can take.
    WaitingOnYou(String),
    /// The agent is on its last item, then its handoff.
    WindingDown(String),
    /// The worktree is kept on purpose with no work running; the text says
    /// why and until when.
    Held(String),
    /// What `builds claim` and `builds new` set on a claim for an item that
    /// `builds item take` has not started there: take clears it, `builds note`
    /// never sets it, and health releases no claim that carries it
    /// (health_fix.rs). Its text is fixed.
    Starting,
}

impl Note {
    /// The note <text> with the state named <state> (one of `NOTE_STATES`), or
    /// no state when <state> is None.
    pub fn with_state(state: Option<&str>, text: String) -> Result<Note> {
        Ok(match state {
            None => Note::Plain(text),
            Some("waiting-on-you") => Note::WaitingOnYou(text),
            Some("winding-down") => Note::WindingDown(text),
            Some("held") => Note::Held(text),
            Some("starting") => Note::Starting,
            Some(other) => fail!(
                "a note's state is not one of {}: {other:?}",
                NOTE_STATES.join(", ")
            ),
        })
    }

    /// The note of <text> and <state> as a claim file or JSON holds them:
    /// None when both are absent, and a refusal for a state with no note.
    pub fn from_fields(text: Option<String>, state: Option<String>) -> Result<Option<Note>> {
        match (text, state) {
            (Some(text), state) => Note::with_state(state.as_deref(), text).map(Some),
            (None, None) => Ok(None),
            (None, Some(_)) => fail!("a note's state needs a note"),
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Note::Plain(text)
            | Note::WaitingOnYou(text)
            | Note::WindingDown(text)
            | Note::Held(text) => text,
            Note::Starting => STARTING_TEXT,
        }
    }

    /// The state's name in the claim file and `builds status --json`.
    pub fn state(&self) -> Option<&'static str> {
        match self {
            Note::Plain(_) => None,
            Note::WaitingOnYou(_) => Some("waiting-on-you"),
            Note::WindingDown(_) => Some("winding-down"),
            Note::Held(_) => Some("held"),
            Note::Starting => Some("starting"),
        }
    }
}

/// A claim, with its note and the note's state in JSON as two fields.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "ClaimJson", into = "ClaimJson")]
pub struct Claim {
    pub who: String,
    /// When the claim began, to the minute.
    pub since: i64,
    pub note: Option<Note>,
}

impl Claim {
    /// The note's state, when the claim has a note with one.
    pub fn state(&self) -> Option<&'static str> {
        self.note.as_ref().and_then(Note::state)
    }

    /// The note's text, when the claim has one.
    pub fn note_text(&self) -> Option<&str> {
        self.note.as_ref().map(Note::text)
    }

    /// Is the worktree held on purpose (`builds note --state held`)?
    pub fn is_held(&self) -> bool {
        matches!(self.note, Some(Note::Held(_)))
    }

    /// Does the claim wait for its item (`Note::Starting`)?
    pub fn is_starting(&self) -> bool {
        matches!(self.note, Some(Note::Starting))
    }
}

/// A claim as JSON: `since` is the `YYYY-MM-DD HH:MM` text that the claim
/// file and `builds status` show.
#[derive(serde::Serialize, serde::Deserialize)]
struct ClaimJson {
    who: String,
    #[serde(with = "minute_text")]
    since: i64,
    note: Option<String>,
    note_state: Option<String>,
}

impl From<Claim> for ClaimJson {
    fn from(claim: Claim) -> ClaimJson {
        ClaimJson {
            note_state: claim.state().map(str::to_string),
            note: claim.note_text().map(str::to_string),
            who: claim.who,
            since: claim.since,
        }
    }
}

impl TryFrom<ClaimJson> for Claim {
    type Error = String;

    fn try_from(json: ClaimJson) -> std::result::Result<Claim, String> {
        let note = Note::from_fields(json.note, json.note_state).map_err(|fault| fault.text())?;
        Ok(Claim {
            who: json.who,
            since: json.since,
            note,
        })
    }
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
        Ok(Self::read_open(file)?.map(|(_, claim)| claim))
    }

    /// The claim and the last write of that same file.
    pub fn read_with_time(file: &Path) -> Result<Option<(Claim, i64)>> {
        use std::os::unix::fs::MetadataExt;
        let Some((opened, claim)) = Self::read_open(file)? else {
            return Ok(None);
        };
        let modified = opened
            .metadata()
            .context(|| format!("reading the modification time of {}", file.display()))?
            .mtime();
        Ok(Some((claim, modified)))
    }

    fn read_open(file: &Path) -> Result<Option<(std::fs::File, Claim)>> {
        use std::io::Read;
        let mut opened = match std::fs::File::open(file) {
            Ok(opened) => opened,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => fail!("reading {}: {error}", file.display()),
        };
        let mut text = String::new();
        opened
            .read_to_string(&mut text)
            .context(|| format!("reading {}", file.display()))?;
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
        let note =
            Note::from_fields(field(fields.next()), field(fields.next())).map_err(|fault| {
                fail::Fail::Message(format!("reading {}: {}", file.display(), fault.text()))
            })?;
        Ok(Some((opened, Claim { who, since, note })))
    }

    pub fn write(&self, file: &Path) -> Result<()> {
        crate::whole_file::write(file, self.line()?)
    }

    /// The claim file's line.
    fn line(&self) -> Result<String> {
        let mut line = format!("{}\t{}", self.who, local_minute(self.since));
        if let Some(note) = &self.note {
            line += &format!("\t{}", note.text());
            if let Some(state) = note.state() {
                line += &format!("\t{state}");
            }
        }
        line.push('\n');
        Ok(line)
    }

    /// Writes the claim beside <file>, for `Staged::put` to move over
    /// <file> (`whole_file::stage`).
    pub fn stage(&self, file: &Path) -> Result<crate::whole_file::Staged> {
        crate::whole_file::stage(file, self.line()?)
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

/// Claims <top> for <who> from <now>, with <note> when given.
pub fn write_claim(checkout: &Checkout, who: &str, now: i64, note: Option<Note>) -> Result<()> {
    Claim {
        who: who.to_string(),
        since: now,
        note,
    }
    .write(&checkout.claim_file())
}

/// Records the project <checkout>'s work is in, or with None forgets it. The
/// project must hold the worktree's repo.
pub fn set_project(places: &Places, checkout: &Checkout, project: Option<&str>) -> Result<()> {
    let file = checkout.project_file();
    let Some(project) = project else {
        return remove_if_present(&file).map(drop);
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
    crate::whole_file::write(&file, format!("{project}\n"))
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
    let kept_note = match Claim::read(&checkout.claim_file())? {
        Some(claim) if claim.who != *who => fail!(
            "{} is claimed by {}; builds release it first if that work is over",
            checkout.top.display(),
            claim.text()
        ),
        Some(claim) => claim.note,
        None => Some(Note::Starting),
    };
    match project {
        Some(project) => set_project(places, &checkout, Some(project))?,
        None => {
            let kept = checkout.claimed_project()?;
            require_project_named(places, &checkout.repo(), kept.as_deref())?;
        }
    }
    write_claim(&checkout, who, now, kept_note)?;
    note(format!("claimed {} for {who}", checkout.top.display()));
    Ok(())
}

/// `builds note <checkout> [--state <state>] [<text>]`: sets the claim's
/// note to the rest of the arguments joined by spaces, with the state
/// --state names or none, or clears note and state when there is no text. The
/// claim's date stays. --state held is refused unless every item running in
/// the worktree waits on an open record (lists/held.rs). A note that makes
/// the worktree held, or ends its hold, starts or ends at <now> the `held`
/// span of each item that runs there (lists/held.rs).
pub fn cmd_note(places: &Places, args: &[String], now: i64) -> Result<()> {
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
    let was_held = claim.is_held();
    let held = state.as_deref() == Some("held");
    let stamp = crate::lists::Stamp {
        at: now,
        by: claim.who.clone(),
    };
    let noted = Claim {
        note: if text.is_empty() {
            None
        } else {
            Some(Note::with_state(state.as_deref(), text)?)
        },
        ..claim
    };
    if was_held == held {
        noted.write(&file)?;
    } else {
        crate::lists::held::record_hold(places, &checkout, held, &stamp, &noted)?;
    }
    note(message);
    Ok(())
}

/// `builds release <checkout>`, refused while an item of any project runs in
/// the worktree, since the claim names the item's agent, and while the
/// worktree holds work origin/main lacks that no item holds. `builds new`
/// reads the same facts (kept.rs).
pub fn cmd_release(places: &Places, args: &[String]) -> Result<()> {
    let [checkout] = args else {
        fail!("usage: builds release <checkout>")
    };
    let checkout = Checkout::find(checkout)?;
    let every = crate::lists::store::every_lists(places)?;
    let commits = crate::procs::git_commits(&crate::procs::Processes::read()?);
    let kept = crate::kept::read(&every, &checkout, &commits)?;
    kept.require_leavable("release", &checkout.top)?;
    match forget(&checkout)? {
        Some(claim) => note(format!(
            "released {} (was {}){}",
            checkout.top.display(),
            claim.text(),
            crate::unlanded::copies_words(&kept.unheld.copied_on)
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
    remove_if_present(&checkout.project_file())?;
    unclaim(checkout)
}

/// `forget`, but only while the claim on <checkout> is still <expected>, the
/// claim the caller checked: another command may have held, replaced or
/// released it since. Returns None when it is no longer <expected>.
pub fn forget_claimed(checkout: &Checkout, expected: &Claim) -> Result<Option<Claim>> {
    if Claim::read(&checkout.claim_file())?.as_ref() != Some(expected) {
        return Ok(None);
    }
    forget(checkout)
}

/// Removes <checkout>'s claim and keeps its project file, and returns the
/// claim, or None when there was none: an item kept there (lists/kept_work.rs)
/// keeps the worktree in its project for the agent that takes it next.
/// unclaim moves the claim to the worktree's last claim file, and kept.rs
/// reads the time that claim began while the worktree has no claim.
pub fn unclaim(checkout: &Checkout) -> Result<Option<Claim>> {
    let file = checkout.claim_file();
    let Some(claim) = Claim::read(&file)? else {
        return Ok(None);
    };
    let last = checkout.last_claim_file();
    std::fs::rename(&file, &last).map_err(|error| {
        fail::Fail::Message(format!(
            "moving {} to {}: {error}",
            file.display(),
            last.display()
        ))
    })?;
    Ok(Some(claim))
}

#[cfg(test)]
#[path = "claim_tests.rs"]
mod tests;

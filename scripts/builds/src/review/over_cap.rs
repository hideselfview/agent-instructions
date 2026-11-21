//! The branches that add more lines than the review's cap
//! (`check::MAX_ADDED_LINES`): what `builds review` records of each and what
//! `builds land` reads. A branch of design files or of renamed identifiers
//! needs no review. Any other waits on a question to the person, who decides
//! whether it is reviewed.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::question::{self, Effect, NewQuestion, QuestionKind};
use crate::lists::record::{self as lists_record, How, Kind};
use crate::lists::{Id, Stamp};
use crate::review::marked_mechanical::Call;
use crate::review::mechanical::Mechanical;
use crate::review::record::Change;

/// Where a branch over the cap stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The review does not run, for the reason <Waiting> gives.
    Waiting(Waiting),
    /// The person answered the question: the review runs.
    Approved(Id),
}

/// Why a branch over the cap is not reviewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waiting {
    /// The change needs no review.
    Mechanical(Mechanical),
    /// The person has not answered the question about it yet.
    Asked(Id),
    /// The person closed the question without approving the review.
    Declined(Id),
}

/// The record of a branch's net change, named by its patch id.
#[derive(Clone, Copy, Debug)]
pub struct Branch<'a> {
    pub change: Change<'a>,
    pub patch_id: &'a str,
}

/// What the store says of <branch> over the cap, if it recorded it.
pub fn standing(db: &Connection, branch: Branch) -> Result<Option<Standing>> {
    let row: Option<(Option<String>, Option<Id>)> = db
        .query_row(
            "SELECT mechanical, question_id FROM review_over_cap WHERE repo = ?1 AND key = ?2 AND patch_id = ?3",
            params![branch.change.repo, branch.change.key, branch.patch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .sql()?;
    let Some((mechanical, question)) = row else {
        return Ok(None);
    };
    if let Some(word) = mechanical {
        let known = Mechanical::parse(&word).expect("the table holds a known word");
        return Ok(Some(Standing::Waiting(Waiting::Mechanical(known))));
    }
    let id = question.expect("the table holds a mechanical word or a question");
    Ok(Some(match lists_record::standing(db, id)? {
        Some((Kind::Question, None)) => Standing::Waiting(Waiting::Asked(id)),
        Some((Kind::Question, Some(How::Answered(_)))) => Standing::Approved(id),
        Some((Kind::Question, Some(_))) => Standing::Waiting(Waiting::Declined(id)),
        other => fail!("review_over_cap names {id} as a question, and the store has {other:?}"),
    }))
}

/// Records <branch> as needing no review for <why>.
pub fn record_mechanical(
    db: &Connection,
    branch: Branch,
    why: Mechanical,
    stamp: &Stamp,
) -> Result<()> {
    insert(db, branch, stamp, Some(why.word()), None)
}

/// Records <branch>, which adds <added> lines, and asks the person whether
/// it is reviewed. Returns the question.
pub fn record_asked(db: &Connection, branch: Branch, added: i64, stamp: &Stamp) -> Result<Id> {
    let change = branch.change;
    let id = question::ask(
        db,
        NewQuestion {
            title: format!(
                "Review {} in {}, which adds {added} lines, over the review cap of {}?",
                change.key,
                change.repo,
                crate::review::check::MAX_ADDED_LINES
            ),
            details: format!(
                "Branch {} of repo {} adds {added} lines, and a review sends its reviewers at most {} added lines, so `builds review` started no reviewer. The branch is no design change and no rename, so `builds land` takes it with no review only when you approve one. Answer this question to have the branch reviewed, which costs one reviewer process for each group that covers its files. Drop it to leave the branch unreviewed and unlanded.",
                change.key,
                change.repo,
                crate::review::check::MAX_ADDED_LINES
            ),
            kind: QuestionKind::Question,
            effect: Effect::SlowsAgents,
            blocks_nothing: Some(format!(
                "builds land waits on it for branch {} of repo {}",
                change.key, change.repo
            )),
            first: None,
            plan: None,
            origin: None,
        },
        stamp,
    )?;
    insert(db, branch, stamp, None, Some(id))?;
    Ok(id)
}

fn insert(
    db: &Connection,
    branch: Branch,
    stamp: &Stamp,
    mechanical: Option<&str>,
    question: Option<Id>,
) -> Result<()> {
    db.execute(
        "INSERT INTO review_over_cap (repo, key, patch_id, at, mechanical, question_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            branch.change.repo,
            branch.change.key,
            branch.patch_id,
            stamp.at,
            mechanical,
            question
        ],
    )
    .sql()?;
    Ok(())
}

/// What `builds review check-landing` says when a branch over the cap needs
/// no review, and what land prints then.
pub const TAKEN: &str = "over the review cap:";

pub fn taken_text(added: i64, why: &Needs, patch_id: &str) -> String {
    match why {
        Needs::Called(call) => format!(
            "mechanical work: the branch adds {added} lines, {} called it mechanical, and {} covers it, so builds land takes it with no review while its patch id stays {patch_id}",
            call.by, call.checked_by
        ),
        Needs::Mechanical(why) => format!(
            "{TAKEN} the branch adds {added} lines, over the cap of {}, and {}, so builds land takes it with no review while its patch id stays {patch_id}",
            crate::review::check::MAX_ADDED_LINES,
            why.says()
        ),
    }
}

/// Why a branch needs no review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Needs {
    /// `builds review` found it mechanical when it was over the cap.
    Mechanical(Mechanical),
    /// An agent called it mechanical work.
    Called(Call),
}

/// What `builds review` and `builds review check-landing` say of a branch
/// over the cap that waits on the person.
pub fn waiting_text(branch: &str, added: i64, waiting: Waiting) -> String {
    let cap = crate::review::check::MAX_ADDED_LINES;
    match waiting {
        Waiting::Asked(id) => format!(
            "{branch} adds {added} lines, over the review cap of {cap}, and is no design change or rename, so no reviewer started. Question {id} asks the person whether to review it, and builds land waits on their answer.\n"
        ),
        Waiting::Declined(id) => format!(
            "{branch} adds {added} lines, over the review cap of {cap}. The person closed question {id} without approving a review, so builds starts no reviewer and builds land takes no commits of it.\n"
        ),
        Waiting::Mechanical(why) => format!(
            "{branch} adds {added} lines, over the review cap of {cap}, and {}, so no reviewer started and builds land takes it with no review.\n",
            why.says()
        ),
    }
}

/// The patch id git gives <diff>, the same for a rebase that keeps its lines.
pub fn patch_id(diff: &str) -> Result<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("git")
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| crate::fail::Fail::Message(format!("running git patch-id: {error}")))?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(diff.as_bytes())
        .map_err(|error| crate::fail::Fail::Message(format!("writing to git patch-id: {error}")))?;
    let output = child
        .wait_with_output()
        .map_err(|error| crate::fail::Fail::Message(format!("running git patch-id: {error}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    match text.split_whitespace().next() {
        Some(id) if output.status.success() => Ok(id.to_string()),
        _ => crate::fail!("git patch-id gave no id for a diff that adds lines"),
    }
}

#[cfg(test)]
#[path = "over_cap_tests.rs"]
mod tests;

//! The branches an agent called mechanical work (`builds review --kind
//! mechanical`, marked_mechanical.sql): moved or renamed code, regenerated
//! files or a codemod, which the compiler, the hook and a machine check cover.
//! `builds land` takes such a branch with no review.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::lists::Stamp;
use crate::review::commits::Unread;
use crate::review::record::Change;

/// Records that <by> called the net change of <worktree> past <base>
/// mechanical work (`builds review --kind mechanical`): moved or renamed
/// code, regenerated files or a codemod, covered by <checked_by>, such as the
/// compiler, the hook or a shots run. The net change leaves out the <unread>
/// folders. `builds land` takes the change with no review. Returns what to
/// say.
pub fn call_mechanical(
    store: &crate::lists::store::Store,
    repo: &str,
    worktree: &std::path::Path,
    (base, unread): (&str, &Unread),
    key: &str,
    call: (&str, &str),
    stamp: &Stamp,
) -> Result<String> {
    let (by, checked_by) = call;
    let net = crate::review::run::diff_between(worktree, base, "HEAD", unread)?;
    let added = crate::review::check::added_lines(net.diff.patch.text());
    let patch_id = patch_id(net.diff.patch.text())?;
    let branch = Branch {
        change: Change { repo, key },
        patch_id: &patch_id,
    };
    let db = store.open_existing()?;
    if call_of(&db, branch)?.is_some() {
        return Ok(format!("{key} is called mechanical work already\n"));
    }
    store.write(stamp, &format!("call mechanical {key}"), |db| {
        db.execute(
            "INSERT INTO review_marked_mechanical (repo, key, patch_id, at, by, checked_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![repo, key, patch_id, stamp.at, by, checked_by],
        )
        .sql()?;
        Ok(())
    })?;
    Ok(format!(
        "{key} adds {added} lines and is called mechanical work checked by {checked_by}, so no reviewer started and builds land takes it with no review while its patch id stays {patch_id}\n"
    ))
}

/// A branch's net change, named by its patch id: a rebase that keeps its
/// lines keeps the patch id.
#[derive(Clone, Copy, Debug)]
pub struct Branch<'a> {
    pub change: Change<'a>,
    pub patch_id: &'a str,
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

/// What `builds review check-landing` says when a branch is called mechanical
/// work, and what land prints then.
pub const TAKEN: &str = "mechanical work:";

/// What the call lets `builds land` do with a branch of <added> lines.
pub fn taken_text(added: i64, call: &Call, patch_id: &str) -> String {
    format!(
        "{TAKEN} the branch adds {added} lines, {} called it mechanical, and {} covers it, so builds land takes it with no review while its patch id stays {patch_id}",
        call.by, call.checked_by
    )
}

/// An agent's call that a branch is mechanical work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// The agent that made the call.
    pub by: String,
    /// The check that covers the change.
    pub checked_by: String,
}

/// The call that <branch> is mechanical work, if an agent made it.
pub fn call_of(db: &Connection, branch: Branch) -> Result<Option<Call>> {
    db.query_row(
        "SELECT by, checked_by FROM review_marked_mechanical WHERE repo = ?1 AND key = ?2 AND patch_id = ?3",
        params![branch.change.repo, branch.change.key, branch.patch_id],
        |row| {
            Ok(Call {
                by: row.get(0)?,
                checked_by: row.get(1)?,
            })
        },
    )
    .optional()
    .sql()
}

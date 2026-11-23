//! The branches an agent called mechanical work (`builds review --kind
//! mechanical`, marked_mechanical.sql): moved or renamed code, regenerated
//! files or a codemod, which the compiler, the hook and a machine check cover.
//! `builds land` takes such a branch with no review.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::Stamp;
use crate::review::over_cap::{Branch, Standing, Waiting, patch_id, standing};
use crate::review::record::Change;

/// Records that <by> called the net change of <worktree> past <base>
/// mechanical work (`builds review --kind mechanical`): moved or renamed
/// code, regenerated files or a codemod, covered by <checked_by>, such as the
/// compiler, the hook or a shots run. `builds land` takes the change with no
/// review. Refuses a change with an open or approved question to the person
/// about reviewing it. Returns what to say.
pub fn call_mechanical(
    store: &crate::lists::store::Store,
    repo: &str,
    worktree: &std::path::Path,
    base: &str,
    key: &str,
    call: (&str, &str),
    stamp: &Stamp,
) -> Result<String> {
    let (by, checked_by) = call;
    let net = crate::review::run::diff_between(worktree, base, "HEAD")?;
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
    match standing(&db, branch)? {
        Some(Standing::Waiting(Waiting::Mechanical(why))) => {
            return Ok(format!("{key} {}\n", why.says()));
        }
        Some(Standing::Waiting(Waiting::Asked(id)) | Standing::Approved(id)) => {
            fail!(
                "{key} already has question {id} about reviewing it, so the person decides it, and a call of mechanical work does not replace it"
            )
        }
        // A question the person closed without approving leaves nothing to decide.
        Some(Standing::Waiting(Waiting::Declined(_))) | None => {}
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

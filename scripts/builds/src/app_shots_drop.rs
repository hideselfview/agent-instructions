//! `builds app-build --drop-shots <repo> <branch> <lane> <prefix> --why TEXT`:
//! an agent's record that the landing of <branch> needs no run of one shots
//! prefix that the branch's own app builds ran. `builds land` requires the
//! app build that covers a branch to have run every shots prefix the
//! branch's app builds ran since its merge base (app_landing.rs), except the
//! prefixes recorded here, and prints each one it honors with its reason.
//!
//! The table (app_shots_drop.sql) is this file's: no other code writes it.

use rusqlite::{Connection, params};

use crate::app_landing::branch_since;
use crate::args::Args;
use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;
use crate::lists::Stamp;
use crate::lists::store::Store;
use crate::output::note;
use crate::tool::Tool;

/// The flag of `builds app-build` that names this command.
pub const FLAG: &str = "--drop-shots";

/// A shots prefix dropped from a branch's landing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drop {
    pub prefix: String,
    pub by: String,
    pub why: String,
}

/// The prefixes of <lane> dropped from the landing of <branch> of <repo> at
/// or after <since> (seconds since 1970), oldest first. A drop older than
/// the branch's first commit belongs to another branch of the same name.
pub fn dropped(
    db: &Connection,
    repo: &str,
    branch: &str,
    lane: Lane,
    since: i64,
) -> Result<Vec<Drop>> {
    let mut statement = db
        .prepare(
            "SELECT prefix, by, why FROM app_shots_drop
             WHERE repo = ?1 AND branch = ?2 AND lane = ?3 AND at >= ?4 ORDER BY id",
        )
        .sql()?;
    let rows = statement
        .query_map(params![repo, branch, lane.word(), since], |row| {
            Ok(Drop {
                prefix: row.get(0)?,
                by: row.get(1)?,
                why: row.get(2)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

impl crate::app_timing::BuildRecords<'_> {
    /// `dropped` of every store, since each drop is recorded in the store of
    /// the project its command named.
    pub fn dropped(&self, branch: &str, lane: Lane, since: i64) -> Result<Vec<Drop>> {
        let mut found = Vec::new();
        for store in &self.stores {
            found.extend(dropped(&store.conn, self.repo, branch, lane, since)?);
        }
        Ok(found)
    }
}

/// Records that the writer of <stamp> dropped <prefix> of <lane> from the
/// landing of <branch> of <repo>, for <why>.
pub fn record(
    db: &Connection,
    (repo, branch): (&str, &str),
    lane: Lane,
    prefix: &str,
    why: &str,
    stamp: &Stamp,
) -> Result<()> {
    db.execute(
        "INSERT INTO app_shots_drop (repo, branch, lane, prefix, at, by, why) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![repo, branch, lane.word(), prefix, stamp.at, stamp.by, why],
    )
    .sql()?;
    Ok(())
}

/// Makes `app_shots_drop` again as app_shots_drop.sql writes it, which takes
/// any app's name as a drop's lane: a store from before took only `mac` and
/// `android`. Each drop keeps its row.
pub fn allow_named_lanes(db: &Connection) -> Result<()> {
    use crate::db::{remake_table, statement};
    remake_table(
        db,
        "app_shots_drop",
        statement(
            include_str!("app_shots_drop.sql"),
            "CREATE TABLE app_shots_drop",
            ") STRICT;",
        ),
        |db| {
            db.execute_batch(
                "INSERT INTO app_shots_drop (id, repo, branch, lane, prefix, at, by, why) SELECT id, repo, branch, lane, prefix, at, by, why FROM app_shots_drop_old",
            )
            .sql()
        },
        &[],
    )
}

/// `builds app-build --drop-shots`: records the drop of <prefix>, which the
/// app builds of <branch> in <lane> ran since the branch left origin/main,
/// and says so. Runs in the branch's worktree.
pub fn cmd_drop_shots(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = &tool.dir;
    let args = Args::parse(args, &["why"], &[])?;
    let [repo, branch, lane, prefix] =
        args.exactly(4, &crate::command::Command::AppBuild.usage_text())?
    else {
        unreachable!("exactly four")
    };
    let lane = crate::repo_lanes::parse(&tool.places, repo, lane)?;
    let why = args.why()?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let Some(checkout) = crate::checkout::Checkout::at(cwd)? else {
        fail!("{} is in no git checkout", cwd.display());
    };
    let since = branch_since(&checkout.top, "origin/main", branch)?;
    let commits = crate::git::commits_in_range(&checkout.top, "origin/main", branch)?;
    let ran = match crate::app_timing::BuildRecords::open(&tool.places, repo)? {
        Some(records) => records.shots_ran(lane, branch, &commits, since)?,
        None => Vec::new(),
    };
    if !ran.contains(prefix) {
        let word = lane.word();
        let said = if ran.is_empty() {
            format!("no app build of {branch} ran any {word} shots")
        } else {
            format!("its {word} shots runs were: {}", ran.join(", "))
        };
        fail!(
            "no app build of {branch} ran the {word} shots prefix {prefix} since the branch left origin/main; {said}"
        );
    }
    store.write(&stamp, &format!("drop shots {prefix} of {branch}"), |db| {
        record(db, (repo, branch), lane, prefix, why, &stamp)
    })?;
    note(format!(
        "builds land will not require a run of the {} shots prefix {prefix} for {branch}, and prints why: {why}",
        lane.word()
    ));
    Ok(())
}

#[cfg(test)]
#[path = "app_shots_drop_tests.rs"]
mod tests;

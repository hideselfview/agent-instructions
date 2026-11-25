//! Accepted verdicts that keep a confirmed finding open.

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::Result;

pub(super) struct Verdict {
    pub finding: i64,
    pub group: String,
    pub run: i64,
    pub reason: String,
}

pub(super) fn record(db: &Connection, call: i64, finding: i64, reason: &str) -> Result<()> {
    db.execute(
        "INSERT INTO review_kept_open (call_id, finding_id, reason) VALUES (?1, ?2, ?3)",
        params![call, finding, reason],
    )
    .sql()?;
    Ok(())
}

pub(super) fn of_landing(db: &Connection, landing: i64) -> Result<Vec<Verdict>> {
    let mut statement = db
        .prepare(
            "SELECT o.finding_id, c.group_name, r.id, o.reason FROM review_run r
             JOIN review_call c ON c.run_id = r.id
             JOIN review_kept_open o ON o.call_id = c.id
             WHERE r.landing_id = ?1
             ORDER BY c.id, o.finding_id",
        )
        .sql()?;
    let rows = statement
        .query_map(params![landing], |row| {
            Ok(Verdict {
                finding: row.get(0)?,
                group: row.get(1)?,
                run: row.get(2)?,
                reason: row.get(3)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// The line that says group <group> kept finding <finding> open, with the
/// reason its reviewer gave.
pub fn text(group: &str, finding: i64, reason: &str) -> String {
    format!(
        "{group} kept finding {finding} open. {}\n",
        crate::text::one_line(reason)
    )
}

/// The reason a store move gives each verdict recorded before builds kept
/// reasons.
pub const UNRECORDED_REASON: &str =
    "builds recorded no reason, since it recorded this verdict before it kept reasons.";

/// Makes `review_kept_open` again as kept_open.sql writes it, with the reason
/// of each verdict: a store from before kept none, and each of its rows gets
/// `UNRECORDED_REASON`.
pub fn add_reason(db: &Connection) -> Result<()> {
    use crate::db::{remake_table, statement};
    let schema = include_str!("kept_open.sql");
    remake_table(
        db,
        "review_kept_open",
        statement(
            schema,
            "CREATE TABLE review_kept_open (",
            ") STRICT, WITHOUT ROWID;",
        ),
        |db| {
            db.execute(
                "INSERT INTO review_kept_open (call_id, finding_id, reason) SELECT call_id, finding_id, ?1 FROM review_kept_open_old",
                params![UNRECORDED_REASON],
            )
            .sql()
            .map(|_| ())
        },
        &[],
    )
}

#[cfg(test)]
#[path = "kept_open_tests.rs"]
pub mod tests;

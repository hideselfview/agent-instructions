//! Accepted verdicts that keep a confirmed finding open.

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::Result;

pub(super) struct Verdict {
    pub finding: i64,
    pub group: String,
    pub run: i64,
}

pub(super) fn record(db: &Connection, call: i64, finding: i64) -> Result<()> {
    db.execute(
        "INSERT INTO review_kept_open (call_id, finding_id) VALUES (?1, ?2)",
        params![call, finding],
    )
    .sql()?;
    Ok(())
}

pub(super) fn of_landing(db: &Connection, landing: i64) -> Result<Vec<Verdict>> {
    let mut statement = db
        .prepare(
            "SELECT o.finding_id, c.group_name, r.id FROM review_run r
             JOIN review_call c ON c.run_id = r.id
             JOIN review_kept_open o ON o.call_id = c.id
             WHERE r.landing_id = ?1",
        )
        .sql()?;
    let rows = statement
        .query_map(params![landing], |row| {
            Ok(Verdict {
                finding: row.get(0)?,
                group: row.get(1)?,
                run: row.get(2)?,
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

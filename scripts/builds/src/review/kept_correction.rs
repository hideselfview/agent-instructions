//! The judge's corrections of the reasons it kept disputed findings
//! (kept_correction.sql). This is the only code that writes the table.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::lists::Stamp;

/// Records <stamp>'s correction of the reason for keeping finding <id>.
pub(super) fn record(db: &Connection, id: i64, why: &str, stamp: &Stamp) -> Result<()> {
    db.execute(
        "INSERT INTO review_kept_correction (finding_id, why, at, by) VALUES (?1, ?2, ?3, ?4)",
        params![id, why, stamp.at, stamp.by],
    )
    .sql()?;
    Ok(())
}

/// The reason and the judge of the latest correction of finding <id>'s keep.
pub(super) fn latest(db: &Connection, id: i64) -> Result<Option<(String, String)>> {
    db.query_row(
        "SELECT why, by FROM review_kept_correction WHERE finding_id = ?1 ORDER BY id DESC LIMIT 1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .sql()
}

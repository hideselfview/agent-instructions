//! Most recent completed work for each repo and reference.

use crate::db::Sql;
use crate::fail::Result;

#[derive(PartialEq, Eq)]
pub struct BranchEnd {
    pub repo: String,
    pub reference: String,
    pub at: i64,
}

impl BranchEnd {
    pub fn read(db: &rusqlite::Connection) -> Result<Vec<Self>> {
        let build_end = crate::app_timing::RUN_END;
        let mut statement = db
            .prepare(&format!(
                "SELECT repo, reference, MAX(at) FROM (
               SELECT r.repo, r.built_ref AS reference, {build_end} AS at FROM app_build_runs r
               UNION ALL SELECT repo, branch, started_at + ms / 1000 FROM land_run
                 WHERE branch IS NOT NULL
             ) GROUP BY repo, reference ORDER BY repo, reference"
            ))
            .sql()?;
        let rows = statement
            .query_map([], |row| {
                Ok(Self {
                    repo: row.get(0)?,
                    reference: row.get(1)?,
                    at: row.get(2)?,
                })
            })
            .sql()?;
        rows.map(|row| row.sql()).collect()
    }
}

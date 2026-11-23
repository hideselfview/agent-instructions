//! The end of work done for a branch.

#[derive(PartialEq, Eq)]
pub struct BranchEnd {
    pub repo: String,
    pub reference: String,
    pub at: i64,
}

impl BranchEnd {
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            repo: row.get(0)?,
            reference: row.get(1)?,
            at: row.get(2)?,
        })
    }
}

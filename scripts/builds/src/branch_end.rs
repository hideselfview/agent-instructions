//! The end of work done for a branch.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchEnd {
    pub repo: String,
    pub reference: String,
    pub at: i64,
}

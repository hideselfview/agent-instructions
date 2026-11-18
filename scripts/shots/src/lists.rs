//! The three lists a run ends with: the boards the design marks ready that no
//! harness shoots (to build), the shots whose board changed since the pair was
//! confirmed (to update), and the shots with no board (to delete).

use crate::lint::Problem;
use crate::names::Name;
use crate::record::Record;
use std::collections::BTreeMap;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Lists {
    pub(crate) to_build: Vec<String>,
    pub(crate) to_update: Vec<String>,
    pub(crate) to_delete: Vec<String>,
}

/// The lists for the declared shots, given the boards marked ready, the
/// record, the hash of each declared shot's board (absent for a shot with no
/// board) and the boards' problems.
pub(crate) fn lists(
    declared: &[Name],
    ready: &[String],
    record: &Record,
    board_hashes: &BTreeMap<Name, String>,
    problems: &[Problem],
) -> Lists {
    let shot_names: Vec<String> = declared.iter().map(Name::to_string).collect();
    let mut to_build: Vec<String> = ready
        .iter()
        .filter(|board| !shot_names.contains(board))
        .cloned()
        .collect();
    to_build.sort();
    to_build.dedup();
    let to_update = declared
        .iter()
        .filter(|name| {
            let recorded = record.get(&name.to_string());
            let current = board_hashes.get(name);
            matches!((recorded, current), (Some(entry), Some(hash)) if &entry.board != hash)
        })
        .map(Name::to_string)
        .collect();
    let to_delete = problems
        .iter()
        .filter(|p| p.declared)
        .map(|p| p.name.clone())
        .collect();
    Lists {
        to_build,
        to_update,
        to_delete,
    }
}

#[cfg(test)]
#[path = "lists_tests.rs"]
mod tests;

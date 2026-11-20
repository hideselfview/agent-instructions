//! The two lists a run ends with: the boards the design marks ready that no
//! harness shoots (to build), and the shots with no board (to delete).

use crate::lint::Problem;
use crate::names::Name;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Lists {
    pub(crate) to_build: Vec<String>,
    pub(crate) to_delete: Vec<String>,
}

/// The lists for the declared shots, given the boards marked ready and the
/// shots' problems.
pub(crate) fn lists(declared: &[Name], ready: &[String], problems: &[Problem]) -> Lists {
    let shot_names: Vec<String> = declared.iter().map(Name::to_string).collect();
    let mut to_build: Vec<String> = ready
        .iter()
        .filter(|board| !shot_names.contains(board))
        .cloned()
        .collect();
    to_build.sort();
    to_build.dedup();
    let to_delete = problems.iter().map(|p| p.name.clone()).collect();
    Lists {
        to_build,
        to_delete,
    }
}

#[cfg(test)]
#[path = "lists_tests.rs"]
mod tests;

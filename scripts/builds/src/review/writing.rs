//! `builds review writing`: asks the group that holds the rule
//! write-plain-literal-active-sentences over a worktree's net change past
//! its base, the way `builds review` asks each group (`run::ask_groups`), and
//! prints the findings. The check records nothing: no landing, no run and no
//! finding go in the store, so an implementing agent runs it on its own
//! branch before it reports, and fixes what it finds.

use std::path::Path;

use crate::fail;
use crate::fail::Result;
use crate::review::render;
use crate::review::run;

/// The rule whose group the writing check asks.
pub const WRITING_RULE: &str = "write-plain-literal-active-sentences";

/// What the writing check found.
#[derive(Debug)]
pub struct Checked {
    /// Everything to print.
    pub text: String,
    /// Whether the group answered and raised no finding that builds kept.
    pub clean: bool,
}

/// Asks the group of <root> that holds `WRITING_RULE`, with <project>'s
/// rules, over the net change of <worktree>'s HEAD past <base>, and records
/// nothing.
pub fn check(
    worktree: &Path,
    base: &str,
    root: &Path,
    project: &str,
    reviewer: &Path,
) -> Result<Checked> {
    let material = run::Material::load(root, project)?;
    let Some(group) = material
        .groups
        .groups
        .iter()
        .find(|group| group.rules.iter().any(|rule| rule == WRITING_RULE))
    else {
        fail!("no group of {} holds rule {WRITING_RULE}", root.display());
    };
    let diff = run::diff_of(worktree, base)?.diff;
    if diff.text.trim().is_empty() {
        return Ok(Checked {
            text: format!("there are no changes past {base}, so there is nothing to check\n"),
            clean: true,
        });
    }
    let picked: Vec<_> = material
        .groups
        .pick(&material.rules, &diff.changed)
        .into_iter()
        .filter(|(picked, _)| picked.name == group.name)
        .collect();
    if picked.is_empty() {
        return Ok(Checked {
            text: format!(
                "{}: its rules cover none of the changed paths, so there is nothing to check\n",
                group.name
            ),
            clean: true,
        });
    }
    let asked = run::ask_groups(&material, reviewer, &picked, &diff, None, &[]);
    let clean = asked.calls.iter().all(|call| {
        call.failure.is_none() && call.findings.iter().all(|f| f.check_failed.is_some())
    });
    Ok(Checked {
        text: render::unrecorded_report(&asked.calls),
        clean,
    })
}

#[cfg(test)]
#[path = "writing_tests.rs"]
mod tests;

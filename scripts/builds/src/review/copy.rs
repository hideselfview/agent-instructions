//! `builds review copy`: the copy review. It asks the copy groups
//! (`groups::Group::copy`), whose rules judge word choice, grammar and style,
//! over a worktree's net change past its base, and records the run with its
//! calls and findings in the project's store: a run of kind `copy` that no
//! landing holds, labelled with the branch's key. The copy review is no part
//! of `builds review` or `builds land`, and nothing waits on it: the person
//! runs it by hand.

use std::path::Path;

use crate::fail::Result;
use crate::lists::Stamp;
use crate::lists::design_paths::Globs;
use crate::lists::store::Store;
use crate::review::record::{self, Kind, NewRun};
use crate::review::render;
use crate::review::run;

/// What one copy review needs.
pub struct Inputs<'a> {
    pub store: &'a Store,
    pub project: &'a str,
    /// The repo of the worktree, whose design files only the rules that
    /// judge design files read.
    pub repo: &'a str,
    /// The review reads the net change of the worktree's HEAD past <base>,
    /// from their common ancestor.
    pub worktree: &'a Path,
    pub base: &'a str,
    /// The key of the change the worktree holds (`branch:<name>`), which the
    /// store labels the run with.
    pub label: &'a str,
    pub root: &'a Path,
    pub reviewer: &'a crate::review::Program,
    pub stamp: &'a Stamp,
}

/// What the copy review found.
#[derive(Debug)]
pub struct Checked {
    /// Everything to print.
    pub text: String,
    /// Whether every group answered and raised no finding that builds kept.
    pub clean: bool,
}

/// Asks the copy groups of the rules checkout whose rules cover a path the
/// change touches, with the project's rules, and records the run.
pub fn check(inputs: &Inputs) -> Result<Checked> {
    let material = run::Material::load(inputs.root, inputs.project)?;
    let design = Globs::read(&*inputs.store.open_existing()?, inputs.repo)?
        .with_capture_folders(inputs.worktree)?;
    let change = run::diff_of(inputs.worktree, inputs.base)?;
    if change.diff.patch.text().trim().is_empty() {
        return Ok(Checked {
            text: format!(
                "there are no changes past {}, so there is nothing to check\n",
                inputs.base
            ),
            clean: true,
        });
    }
    let picked = material
        .groups
        .pick_copy(&material.rules, &change.diff.changed, &design);
    if picked.is_empty() {
        return Ok(Checked {
            text: "no copy group's rules cover the changed paths, so there is nothing to check\n"
                .to_string(),
            clean: true,
        });
    }
    let scope = run::Scope {
        material: &material,
        files: &design,
    };
    let asked = run::ask_groups(
        scope,
        inputs.reviewer,
        run::ReviewKind::Check,
        &picked,
        &change.diff,
        None,
        &[],
    );
    let stamp = inputs.stamp;
    let (run, calls, findings) =
        inputs
            .store
            .write(stamp, &format!("review copy {}", inputs.label), |db| {
                let (run, _) = record::add_run(
                    db,
                    &inputs.store.texts,
                    &NewRun {
                        landing: None,
                        kind: Kind::Copy,
                        label: Some(inputs.label),
                        at: stamp.at,
                        base: &change.base,
                        head: &change.head,
                        since: None,
                        rules_version: &run::rules_version(inputs.root),
                    },
                    &asked.calls,
                )?;
                Ok((
                    run,
                    record::calls_of_run(db, run)?,
                    record::findings_of_run(db, run)?,
                ))
            })?;
    let clean = calls.iter().all(|call| call.failure.is_none())
        && findings.iter().all(|finding| !finding.counts());
    let mut text = format!(
        "copy review of {} (run {run}) over {}...{}\n",
        inputs.label,
        super::short(&change.base),
        super::short(&change.head)
    );
    text.push_str(&render::run_report(&calls, &findings));
    Ok(Checked { text, clean })
}

#[cfg(test)]
#[path = "copy_tests.rs"]
mod tests;

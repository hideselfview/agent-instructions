//! What `builds health` says of an idle claimed worktree's branch: whether
//! its review lets the commits land, read from the same records and branch
//! `builds land` reads (`landing::judge`).

use crate::health::Advice;
use crate::lists::Lists;
use crate::lists::running::running_in;
use crate::lists::store::Store;
use crate::places::Places;
use crate::review::landing::{self, CodeCheck};
use crate::review::record::Change;
use crate::review::run::Material;

/// What the review says of an idle branch's commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Landing {
    /// The review does not take them yet: the clauses of `landing::blockers`.
    Blocked(Vec<String>),
    /// Nothing blocks landing: the command that lands them.
    Free(String),
}

impl Landing {
    /// What `builds health` says of the branch.
    pub fn advice(&self) -> Advice {
        match self {
            Landing::Blocked(blockers) => Advice::said(format!(
                "its branch cannot land yet: {}",
                blockers.join("; ")
            )),
            Landing::Free(command) => {
                Advice::run("nothing blocks landing: run".to_string(), command.clone())
            }
        }
    }
}

/// What blocks the commits of the worktree <top> of <repo> from landing, for
/// the work in <lists>' project: the clauses of `landing::blockers`, or the
/// command that lands them, which names the items running there. <ahead>
/// holds the full hashes of the commits origin/main lacks, oldest first, and
/// <rules_root> is the checkout of the rules and groups. None when it has
/// none.
pub fn landing(
    places: &Places,
    lists: &Lists,
    top: &std::path::Path,
    repo: &str,
    ahead: &[String],
    rules_root: &std::path::Path,
) -> crate::fail::Result<Option<Landing>> {
    if ahead.is_empty() {
        return Ok(None);
    }
    let project = &lists.project;
    let store = Store::named(places, project)?;
    let conn = store.open_existing()?;
    let material = Material::load(rules_root, project)?;
    let needs = landing::needs(top, ahead.to_vec(), |paths| material.group_names(paths))?;
    let key = super::default_key(top)?;
    let verdict = landing::judge(
        &conn,
        Change { repo, key: &key },
        top,
        &needs,
        CodeCheck::Branch("HEAD"),
        &material,
    )?;
    let blockers = landing::blockers(&verdict);
    if !blockers.is_empty() {
        return Ok(Some(Landing::Blocked(blockers)));
    }
    let mut command = format!("builds land {}", top.display());
    for item in running_in(lists, &top.display().to_string()) {
        command.push_str(&format!(" --done {}", item.id.0));
    }
    for sha in ahead {
        command.push(' ');
        command.push_str(sha);
    }
    Ok(Some(Landing::Free(command)))
}

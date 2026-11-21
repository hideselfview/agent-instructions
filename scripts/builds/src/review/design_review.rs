//! Where the rules review of a design change stands while the change waits on
//! the person's approval (decision #362). The person reviews a change only
//! once the rules review of its branch has read the branch as it is and no
//! finding waits: the review takes the branch's commits as `builds land`
//! would (`landing::judge`). builds stores none of this: each reader reads it
//! from the review's records and the branch, so a commit that changes the
//! branch takes the change off Waiting on you until a review reads it.
//! `builds design record` and `amend` start the review when it has a change
//! to read (`start`). The dashboard reads the same two facts, the design
//! commits the change covers and the review's verdict, through its own
//! caches (daemon/design_cards.rs, review/verdicts.rs).

use std::path::Path;

use crate::ack::agent::Caller;
use crate::fail::{Fail, Result};
use crate::inbox::Messaging;
use crate::lists::design::design_commits;
use crate::lists::design_drawn::{ProgramRenders, Renders};
use crate::lists::design_paths::Globs;
use crate::lists::store::Store;
use crate::lists::{DesignChange, Id, Lists, Stamp};
use crate::review::landing::{self, CodeCheck, Verdict};
use crate::review::record::Change;
use crate::review::run::{Material, Scope};
use crate::tool::Tool;

/// Where the rules review of an open design change that waits on the
/// person's approval stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The review of the change's branch takes its commits: the change waits
    /// on the person's review.
    Read,
    /// The commits that the change's worktree holds past origin/main are not
    /// the ones the change covers: another branch is checked out there, or
    /// the branch draws the change's boards differently since it was
    /// recorded. No review of a branch reads the change.
    Uncovered,
    /// The review does not take the branch's commits yet.
    InReview(Verdict),
}

impl Standing {
    /// Does the review lack a read of the branch, so that `builds review` has
    /// a change to read: no review of it is open, a group gave no answer over
    /// a diff the review read, or the branch changed since the last review?
    pub fn lacks_a_read(&self) -> bool {
        match self {
            Standing::InReview(Verdict::NoRun) => true,
            Standing::InReview(Verdict::Unfinished(_, unfinished)) => {
                !unfinished.unanswered.is_empty() || !unfinished.unreviewed.is_empty()
            }
            _ => false,
        }
    }

    /// Where the review of design change <id>, drawn in <worktree>, stands, in
    /// a line.
    pub fn line(&self, id: Id, worktree: &str) -> String {
        match self {
            Standing::Read => format!(
                "the rules review read design change {id} as the person sees it, and no finding waits: the change waits on the person's review (the Waiting on you tab)\n"
            ),
            Standing::Uncovered => format!(
                "design change {id} is in review: the commits that {worktree} holds past origin/main are not the ones it covers, so no review reads it; check out its branch there, or amend it over the commits\n"
            ),
            Standing::InReview(verdict) => format!(
                "design change {id} is in review, and stays off the Waiting on you tab until its rules review takes the branch: {}\n",
                landing::blockers(verdict).join("; ")
            ),
        }
    }
}

/// Where the rules review of open design change <design> stands, from the
/// commits that its worktree's HEAD holds past origin/main, the review's
/// records in <store>, the groups of <material> and the design files of the
/// change's repo, which <lists> gives. <renders> draws the change's boards
/// when the commits' files differ from the change's (`Sameness`).
pub fn read(
    store: &Store,
    lists: &Lists,
    design: &DesignChange,
    material: &Material,
    renders: &dyn Renders,
) -> Result<Standing> {
    let top = Path::new(&design.worktree);
    let ahead = crate::side_git::read(top)
        .map_err(Fail::Message)?
        .ahead_oldest_first();
    let covers = design_commits(lists, &design.repo, top, &ahead, renders)?
        .iter()
        .any(|commit| commit.covered_by == Some(design.id));
    if !covers {
        return Ok(Standing::Uncovered);
    }
    let design_files = Globs::of(lists, &design.repo).with_capture_folders(top)?;
    let scope = Scope {
        material,
        files: &design_files,
    };
    let needs = landing::needs(top, ahead, |paths| scope.group_names(paths))?;
    let key = super::default_key(top)?;
    let change = Change {
        repo: &design.repo,
        key: &key,
    };
    let conn = store.open_existing()?;
    let verdict = landing::judge(&conn, change, top, &needs, CodeCheck::Branch("HEAD"), scope)?;
    Ok(match verdict {
        Verdict::NotNeeded | Verdict::Takes(_) | Verdict::OverCap(_) => Standing::Read,
        verdict => Standing::InReview(verdict),
    })
}

/// What `builds design record` and `amend` say of open design change <id>,
/// which waits on the person's approval: where its rules review stands, after
/// a review of its branch when the review has a change to read, as
/// `builds review <worktree>` runs it with the rules and reviewer of <tool>.
/// <runner> gives the agent that runs the review, and <own> the inbox of its
/// session (judge.rs).
pub fn start(
    tool: &Tool,
    store: &Store,
    id: Id,
    stamp: &Stamp,
    runner: impl FnOnce() -> Result<Option<Caller>>,
    own: Option<&Messaging>,
) -> Result<String> {
    let material = Material::load(&tool.reviewer.rules_root, &store.project)?;
    let lists = store.load()?;
    let Some(design) = lists.design(id) else {
        crate::fail!("{id} is no open design change");
    };
    let mut standing = read(
        store,
        &lists,
        design,
        &material,
        &ProgramRenders(tool.program.clone()),
    )?;
    let mut out = String::new();
    if standing.lacks_a_read() {
        let worktree = Path::new(&design.worktree);
        out.push_str(&super::review_branch(
            tool,
            store,
            &design.repo,
            worktree,
            stamp,
            runner()?.as_ref(),
            own,
        )?);
        standing = read(
            store,
            &store.load()?,
            design,
            &material,
            &ProgramRenders(tool.program.clone()),
        )?;
    }
    out.push_str(&standing.line(id, &design.worktree));
    Ok(out)
}

#[cfg(test)]
#[path = "design_review_tests.rs"]
mod tests;

//! `builds discard`: throws away work origin/main lacks that no open item
//! holds, either a side worktree's (its commits, the commits of the branches
//! it left, its files not committed) or that of a branch no worktree checks
//! out. An item's work goes with `builds item drop` (lists/dropped.rs). The
//! coordinator decides (`builds help workflow`, WORKTREES).
//!
//! The command records in the project's decision log who threw the work
//! away, why, and each commit and file, before it changes git, because a
//! rollback can't undo git. When git fails, running the same discard again
//! throws away and records what is left.

use std::path::Path;

use crate::args::Args;
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::Result;
use crate::lists::Stamp;
use crate::lists::decision::{NewDecision, add_decision};
use crate::lists::dropped::{clear_worktree, delete_branch};
use crate::lists::store::Store;
use crate::output::note;
use crate::side_git::Commit;
use crate::tool::Tool;

pub fn cmd_discard(tool: &Tool, args: &Args, cwd: &Path) -> Result<()> {
    let [path] = args.positional() else {
        fail!("{}", args.command().usage_text())
    };
    let why = args.why()?;
    let stamp = Stamp::resolve(tool.now, args, cwd)?;
    let checkout = Checkout::find(path)?;
    let _picking = crate::worktree::lock_picking(&checkout.main)?;
    let flag = args.value("project")?;
    match args.value("branch")? {
        Some(branch) => discard_branch(tool, &checkout, branch, flag, why, &stamp),
        None => discard_worktree(tool, &checkout, flag, why, &stamp),
    }
}

/// Throws away the work in the side worktree <checkout> that no item holds,
/// detaches it at origin/main and removes its claim. Refused while an item of
/// any project runs in or keeps the worktree, and while a `git commit` runs
/// there.
fn discard_worktree(
    tool: &Tool,
    checkout: &Checkout,
    flag: Option<&str>,
    why: &str,
    stamp: &Stamp,
) -> Result<()> {
    let top = checkout.top.display().to_string();
    if !checkout.is_side() {
        fail!(
            "discard throws away the work of a side worktree, which {top} is not; --branch B throws away a branch no worktree checks out"
        );
    }
    let every = crate::lists::store::every_lists(&tool.places)?;
    let commits = crate::procs::git_commits(&crate::procs::Processes::read()?);
    let kept = crate::kept::read(&every, checkout, &commits)?;
    let items: Vec<&str> = kept
        .running
        .iter()
        .map(|(_, words)| words.as_str())
        .chain(kept.keeping.iter().map(String::as_str))
        .collect();
    if !items.is_empty() {
        fail!(
            "discard refuses {top}: it holds the work of {}; `builds item drop N --why WHY` throws away an item's work",
            items.join("; ")
        );
    }
    let held = kept.unheld;
    if !held.committing.is_empty() {
        fail!(
            "discard refuses {top}: a git commit runs in it; run discard again once the commit ends"
        );
    }
    if held.git_is_empty() {
        fail!("{top} holds no work to throw away; `builds release {top}` removes its claim");
    }
    let named = crate::unlanded::named_branches(every.iter());
    let repo = checkout.repo();
    let (head_branch, head) = &held.head;
    let branches: Vec<&String> = head_branch
        .iter()
        .filter(|branch| !head.is_empty() && !named.contains(&(repo.clone(), (*branch).clone())))
        .chain(held.left.iter().map(|(branch, _)| branch))
        .collect();
    let mut lines = Vec::new();
    if !head.is_empty() {
        let on = head_branch.as_deref().unwrap_or("a detached HEAD");
        lines.extend(commit_lines(on, head));
    }
    for (branch, commits) in &held.left {
        lines.extend(commit_lines(branch, commits));
    }
    lines.extend(
        held.uncommitted
            .iter()
            .map(|path| format!("file not committed: {path}")),
    );
    record(
        tool,
        checkout,
        flag,
        &format!("Threw away the unlanded work in {top}"),
        why,
        &lines,
        stamp,
    )?;
    clear_worktree(checkout)
        .and_then(|()| {
            branches
                .iter()
                .try_for_each(|branch| delete_branch(&checkout.main, branch))
        })
        .and_then(|()| crate::claim::forget(checkout).map(drop))
        .map_err(|error| rerun(error, &format!("builds discard {top} --why WHY")))?;
    note(format!("threw away the unlanded work in {top}"));
    Ok(())
}

/// Deletes <branch> of <checkout>'s repo, which no worktree checks out and no
/// open item names.
fn discard_branch(
    tool: &Tool,
    checkout: &Checkout,
    branch: &str,
    flag: Option<&str>,
    why: &str,
    stamp: &Stamp,
) -> Result<()> {
    let main = &checkout.main;
    let repo = checkout.repo();
    if !crate::git::branch_exists(main, branch)? {
        fail!("{repo} has no branch {branch}");
    }
    if let Some(holder) = crate::checkout::worktree_list(main)?
        .into_iter()
        .find(|listed| listed.branch.as_deref() == Some(branch))
    {
        let top = holder.top.display();
        fail!(
            "discard refuses {branch}: {top} checks it out; `builds discard {top} --why WHY` throws away that worktree's work"
        );
    }
    let every = crate::lists::store::every_lists(&tool.places)?;
    if crate::unlanded::named_branches(every.iter()).contains(&(repo.clone(), branch.to_string())) {
        fail!(
            "discard refuses {branch}: an open item runs on it or keeps it; `builds item drop N --why WHY` throws away an item's work"
        );
    }
    let commits = crate::side_git::waiting_to_land(main, &format!("refs/heads/{branch}"))
        .map_err(fail::Fail::Message)?;
    if commits.is_empty() {
        fail!("{repo} {branch} holds no commit origin/main lacks");
    }
    // The branch's project comes from the main checkout, which no one claims.
    let main_checkout = Checkout::find(&main.display().to_string())?;
    record(
        tool,
        &main_checkout,
        flag,
        &format!("Threw away the branch {branch} of {repo}"),
        why,
        &commit_lines(branch, &commits),
        stamp,
    )?;
    delete_branch(main, branch).map_err(|error| {
        rerun(
            error,
            &format!(
                "builds discard {} --branch {branch} --why WHY",
                main.display()
            ),
        )
    })?;
    note(format!("threw away the branch {branch} of {repo}"));
    Ok(())
}

/// One line for each commit of <commits> on <on>, with its full id, which git
/// keeps until it removes unreachable objects.
fn commit_lines(on: &str, commits: &[Commit]) -> Vec<String> {
    commits
        .iter()
        .map(|commit| format!("commit on {on}: {} {}", commit.full, commit.subject))
        .collect()
}

/// Records the discard of <lines> as a decision of the project <checkout>'s
/// work is in.
fn record(
    tool: &Tool,
    checkout: &Checkout,
    flag: Option<&str>,
    title: &str,
    why: &str,
    lines: &[String],
    stamp: &Stamp,
) -> Result<()> {
    let Some(project) = crate::lists::store::project_of(&tool.places, checkout, flag)? else {
        fail!(
            "no project holds repo {}, so builds has no decision log to record the discard in",
            checkout.repo()
        );
    };
    let decision = NewDecision {
        title: title.to_string(),
        by: stamp.by.clone(),
        body: format!("{why}\n\n{}", lines.join("\n")),
        question: None,
        replaces: None,
        plan: None,
    };
    let added = Store::named(&tool.places, &project)?.write(stamp, "discard", |db| {
        add_decision(db, &decision, &[], stamp)
    })?;
    note(format!(
        "recorded the discard as decision {}",
        added.decision
    ));
    Ok(())
}

/// <error>, a git step's failure after the record, with the <command> that
/// throws away the rest.
fn rerun(error: fail::Fail, command: &str) -> fail::Fail {
    fail::Fail::Message(format!(
        "the discard is recorded, but git failed: {}; run `{command}` again once that is fixed",
        error.text()
    ))
}

#[cfg(test)]
#[path = "discard_tests.rs"]
mod tests;

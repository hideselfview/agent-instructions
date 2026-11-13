//! `builds item take`: an agent starts work on items. The agent is the
//! worktree's claim, read when asked and never stored; the worktrees,
//! branches and brief are checked before the store's lock (git and the file
//! system), what depends on the lists under it. Work that spans repos names
//! one worktree and branch per repo (`--worktree W --branch B` repeated, the
//! nth branch for the nth worktree); the items run in all of them.

use std::path::Path;
use std::process::Stdio;

use crate::checkout::{Checkout, git};
use crate::claim::Claim;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::Args;
use crate::lists::draft::require_brief_from;
use crate::lists::item::open_item;
use crate::lists::item_command::blocking_words;
use crate::lists::running::{Running, running_in, start};
use crate::lists::store::Store;
use crate::lists::{Audience, Id, Stamp};
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "builds item take N [M...] --worktree W --branch B [--worktree W2 --branch B2] --brief P [--as design-session]";

/// Does branch <branch> exist in the repo of <top>?
fn branch_exists(top: &Path, branch: &str) -> Result<bool> {
    let status = git(top)
        .args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context(|| format!("running git in {}", top.display()))?;
    Ok(status.success())
}

/// One worktree an item runs in: checked, with its branch and the agent that
/// holds it.
struct Place {
    top: String,
    checkout: Checkout,
    branch: String,
    agent: String,
}

pub fn take(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["worktree", "branch", "brief", "as"], &[])?;
    if args.positional().is_empty() {
        fail!("usage: {USAGE}");
    }
    let ids: Vec<Id> = args.positional().iter().map(|id| Id::parse(id)).collect::<Result<_>>()?;
    for (at, id) in ids.iter().enumerate() {
        if ids[..at].contains(id) {
            fail!("{id} is named twice");
        }
    }
    let design_session = match args.value("as")? {
        None => false,
        Some("design-session") => true,
        Some(other) => fail!("--as takes design-session, got '{other}'"),
    };
    let worktrees = args.values("worktree");
    let branches: Vec<String> = args.values("branch").into_iter().map(str::to_string).collect();
    if worktrees.is_empty() {
        fail!("--worktree is required: {USAGE}");
    }
    if worktrees.len() != branches.len() {
        fail!("{} --worktree and {} --branch given; each worktree takes the branch of its work, in the same order", worktrees.len(), branches.len());
    }
    let brief = args.require("brief")?.to_string();

    let mut places: Vec<Place> = Vec::new();
    for (worktree, branch) in worktrees.into_iter().zip(branches) {
        let checkout = Checkout::find(worktree)?;
        if !checkout.is_side() {
            fail!("{} is not one of the repo's side worktrees (<repo>-worktrees/side-N); items run in those", checkout.top.display());
        }
        if let Some(same) = places.iter().find(|place| place.checkout.repo() == checkout.repo()) {
            fail!(
                "{} and {} are both worktrees of {}; an item runs in one worktree per repo",
                same.checkout.top.display(),
                checkout.top.display(),
                checkout.repo()
            );
        }
        let agent = match Claim::read(&checkout.claim_file())? {
            Some(claim) if !claim.who.is_empty() => claim.who,
            _ => fail!(
                "{} has no claim, and the claim is what names the agent: `builds claim {} WHO` first",
                checkout.top.display(),
                checkout.top.display()
            ),
        };
        if !branch_exists(&checkout.top, &branch)? {
            fail!("{} has no branch {branch}", checkout.top.display());
        }
        places.push(Place { top: checkout.top.display().to_string(), checkout, branch, agent });
    }
    require_brief_from(cwd, &brief)?;

    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("item take {}", ids.iter().map(|i| i.0.to_string()).collect::<Vec<_>>().join(" ")), |lists| {
        for Place { top, checkout, branch, .. } in &places {
            if !lists.repos.contains(&checkout.repo()) {
                fail!("{top} is a worktree of repo {}, which is not one of project {}'s ({})", checkout.repo(), lists.project, lists.repos.join(", "));
            }
            if let Some(other) = running_in(lists, top).into_iter().find(|i| i.running.iter().any(|r| r.worktree == *top && r.branch != *branch)) {
                let running = other.running.iter().find(|r| r.worktree == *top).expect("filtered");
                fail!("{top} already runs {} on branch {}, not {branch}: a worktree holds one branch of work", other.id, running.branch);
            }
        }
        for id in &ids {
            let item = open_item(lists, *id)?;
            if let Some(running) = item.running.first() {
                fail!("{id} already runs in {} on {}; `builds item stop {} --why WHY` first", running.worktree, running.branch, id.0);
            }
            match (item.r#for, design_session) {
                (Audience::DesignSession, false) => fail!("{id} is the design session's; take it with --as design-session"),
                (Audience::Coordinator, true) => fail!("{id} is the coordinator's; --as design-session takes only the design session's items"),
                _ => {}
            }
            let blocking = blocking_words(lists, item);
            if !blocking.is_empty() {
                fail!("{id} waits on {}; it can be taken once they are met", blocking.join("; "));
            }
        }
        let running: Vec<Running> = places
            .iter()
            .map(|place| Running {
                worktree: place.top.clone(),
                branch: place.branch.clone(),
                brief: brief.clone(),
                since: stamp.clone(),
            })
            .collect();
        start(lists, &ids, &running)
    })?;
    let named: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    let wheres: Vec<String> = places.iter().map(|p| format!("{} on {} for {}", p.top, p.branch, p.agent)).collect();
    note(format!("{} run in {}", named.join(", "), wheres.join(" and ")));
    Ok(())
}

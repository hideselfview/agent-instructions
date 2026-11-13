//! `builds item take`: an agent starts work on items. The agent is the
//! worktree's claim, read when asked and never stored; the worktree, branch
//! and brief are checked before the store's lock (git and the file system),
//! what depends on the lists under it.

use std::path::Path;
use std::process::Stdio;

use crate::checkout::{Checkout, git};
use crate::claim::Claim;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::Args;
use crate::lists::draft::{brief_base, require_brief};
use crate::lists::item::{blocking_words, open_item};
use crate::lists::running::running_in;
use crate::lists::store::Store;
use crate::lists::{Audience, Id, Running, Stamp};
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "builds item take N [M...] --worktree W --branch B --brief P [--as design-session]";

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
    let worktree = args.require("worktree")?;
    let branch = args.require("branch")?.to_string();
    let brief = args.require("brief")?.to_string();

    let checkout = Checkout::find(worktree)?;
    if !checkout.is_side() {
        fail!("{} is not one of the repo's side worktrees (<repo>-worktrees/side-N); items run in those", checkout.top.display());
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
    require_brief(&brief_base(cwd)?, &brief)?;

    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let top = checkout.top.display().to_string();
    store.with_write(&stamp, &format!("item take {}", ids.iter().map(|i| i.0.to_string()).collect::<Vec<_>>().join(" ")), |lists| {
        if !lists.repos.contains(&checkout.repo()) {
            fail!("{top} is a worktree of repo {}, which is not one of project {}'s ({})", checkout.repo(), lists.project, lists.repos.join(", "));
        }
        if let Some(other) = running_in(lists, &top).into_iter().find(|i| i.running.as_ref().is_some_and(|r| r.branch != branch)) {
            let running = other.running.as_ref().expect("filtered");
            fail!("{top} already runs {} on branch {}, not {branch}: a worktree holds one branch of work", other.id, running.branch);
        }
        for id in &ids {
            let item = open_item(lists, *id)?;
            if let Some(running) = &item.running {
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
        for id in &ids {
            lists.item_mut(*id).expect("open").running = Some(Running {
                worktree: top.clone(),
                branch: branch.clone(),
                brief: brief.clone(),
                since: stamp.clone(),
            });
        }
        Ok(())
    })?;
    let named: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    note(format!("{} run in {top} on {branch} for {agent}", named.join(", ")));
    Ok(())
}

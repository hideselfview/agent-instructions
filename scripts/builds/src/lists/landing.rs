//! `builds item check-landing` and `builds item landed`: what land.sh asks
//! before it fetches anything, and what it records after the push. Both take
//! land.sh's arguments exactly:
//!
//! `check-landing DIR [--done N]... [--part N]... SHA...`
//! `landed DIR [--done N]... [--part N]... SHA...`
//!
//! DIR is the landing worktree. `--done` names an item the commits finish,
//! `--part` one they advance. `check-landing` runs before the commits are
//! fetched or picked, so the SHAs are the agent's branch commits and must be
//! reachable from the branch the named item runs on in DIR's repo (an item
//! that spans repos runs in a worktree of each, and a landing from any of
//! them is accepted; `--part` leaves it running everywhere, `--done` closes
//! it); `landed` runs after the push,
//! so its SHAs are the commits now on origin/main and must be there. Git runs
//! before the store's lock.

use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::checkout::{Checkout, git};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::Args;
use crate::lists::record::close;
use crate::lists::store::Store;
use crate::lists::item::open_item;
use crate::lists::running::{Running, side_worktree_parts};
use crate::lists::{Closed, How, Id, Kind, Lists, Located, Stamp, Standing};
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "builds item check-landing|landed DIR [--done N]... [--part N]... SHA...";

/// Part of an item already on main.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandedPart {
    pub repo: String,
    pub commits: Vec<String>,
    pub at: i64,
}

/// Records on open item <id> that <part> of it is on main.
pub fn add_part(lists: &mut Lists, id: Id, part: LandedPart) -> Result<()> {
    let Some(item) = lists.item_mut(id) else { fail!("{id} is not an open item") };
    item.landed.push(part);
    Ok(())
}

/// An item a landing names, and whether the landing finishes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Named {
    id: Id,
    finishes: bool,
}

struct Landing {
    checkout: Checkout,
    items: Vec<Named>,
    /// Each SHA as given.
    shas: Vec<String>,
    args: Args,
}

fn parse(args: &[String]) -> Result<Landing> {
    let args = Args::parse(args, &["done", "part"], &[])?;
    let Some((dir, shas)) = args.positional().split_first() else { fail!("usage: {USAGE}") };
    if shas.is_empty() {
        fail!("name the commits: {USAGE}");
    }
    let mut items = Vec::new();
    for (flag, finishes) in [("done", true), ("part", false)] {
        for id in args.values(flag) {
            items.push(Named { id: Id::parse(id)?, finishes });
        }
    }
    if items.is_empty() {
        fail!("name the work items this landing finishes (--done N) or advances (--part N)");
    }
    for (at, named) in items.iter().enumerate() {
        if items[..at].iter().any(|other| other.id == named.id) {
            fail!("{} is named twice; a landing finishes or advances an item, not both", named.id);
        }
    }
    let checkout = Checkout::find(dir)?;
    Ok(Landing { checkout, items, shas: shas.to_vec(), args })
}

/// <sha> as the full id of a commit in the repo of <dir>.
fn commit_id(dir: &Path, sha: &str) -> Result<String> {
    let output = git(dir)
        .args(["rev-parse", "--verify", "--quiet", &format!("{sha}^{{commit}}")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context(|| format!("running git in {}", dir.display()))?;
    if !output.status.success() {
        fail!("{sha} is not a commit in {}", dir.display());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Is <commit> reachable from <rev> in <dir>? A failure of git itself (an
/// unknown rev) is an error, not a no.
fn is_ancestor(dir: &Path, commit: &str, rev: &str) -> Result<bool> {
    let output = git(dir)
        .args(["merge-base", "--is-ancestor", commit, rev])
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running git in {}", dir.display()))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => fail!(
            "git could not tell whether {commit} is on {rev} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

/// The place item <id> runs in the repo being landed, the one whose branch
/// the landing's commits are on. An item that runs only in other repos can't
/// land from this one.
fn running_in_repo<'a>(running: &'a [Running], checkout: &Checkout, id: Id) -> Result<&'a Running> {
    let found = running.iter().find(|r| side_worktree_parts(&r.worktree).is_some_and(|(repo, _)| repo == checkout.repo()));
    match found {
        Some(running) => Ok(running),
        None => {
            let worktrees: Vec<&str> = running.iter().map(|r| r.worktree.as_str()).collect();
            fail!("{id} runs in {}, not in a worktree of {}, the repo being landed", worktrees.join(" and "), checkout.repo())
        }
    }
}

pub fn check_landing(tool: &Tool, args: &[String]) -> Result<()> {
    let landing = parse(args)?;
    let store = Store::resolve(&tool.places, &landing.args, &landing.checkout.top)?;
    let lists = store.load()?;
    let mut branches = Vec::new();
    for named in &landing.items {
        let item = open_item(&lists, named.id)?;
        if item.running.is_empty() {
            fail!("{} is not running, and a landing is an agent's branch arriving: `builds item take` it first", named.id);
        }
        let running = running_in_repo(&item.running, &landing.checkout, named.id)?;
        branches.push((named.id, running.worktree.clone(), format!("refs/heads/{}", running.branch)));
    }
    for sha in &landing.shas {
        let commit = commit_id(&landing.checkout.top, sha)?;
        let mut reached = false;
        for (_, worktree, branch) in &branches {
            if is_ancestor(Path::new(worktree), &commit, branch)? {
                reached = true;
                break;
            }
        }
        if !reached {
            let named: Vec<String> =
                branches.iter().map(|(id, _, branch)| format!("{id} ({})", branch.trim_start_matches("refs/heads/"))).collect();
            fail!("{sha} is on the branch of none of the items named: {}", named.join(", "));
        }
    }
    note(format!("the landing checks out against {} item(s)", landing.items.len()));
    Ok(())
}

fn same_set(a: &[String], b: &[String]) -> bool {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort();
    b.sort();
    a == b
}

/// Records on <lists> that <commits> of <repo> landed for <items>; the same
/// call again records nothing more. Returns what each item did.
fn record(lists: &mut Lists, items: &[Named], repo: &str, commits: &[String], stamp: &Stamp) -> Result<Vec<String>> {
    let mut did = Vec::new();
    for named in items {
        let id = named.id;
        match lists.locate(id) {
            None => fail!("{id} is not a record"),
            Some(Located { kind, .. }) if kind != Kind::Item => fail!("{id} is a {}, not an item", kind.name()),
            Some(Located { standing: Standing::Closed(Closed { how: How::Landed(done), .. }), .. }) => {
                if !same_set(done, commits) {
                    fail!("{id} is closed as landed with other commits ({}); a different set of commits is refused", done.join(" "));
                }
                did.push(format!("{id} was already recorded"));
            }
            Some(Located { standing: Standing::Closed(closed), .. }) => {
                fail!("{id} is closed ({}), so no landing records on it", closed.how.label())
            }
            Some(Located { standing: Standing::Open, .. }) => {
                let item = lists.item(id).expect("located open");
                if named.finishes {
                    close(lists, id, How::Landed(commits.to_vec()), stamp)?;
                    did.push(format!("{id} landed"));
                } else if item.landed.iter().any(|part| part.repo == repo && same_set(&part.commits, commits)) {
                    did.push(format!("{id} already had this part"));
                } else {
                    add_part(lists, id, LandedPart { repo: repo.to_string(), commits: commits.to_vec(), at: stamp.at })?;
                    did.push(format!("{id} gained a landed part"));
                }
            }
        }
    }
    Ok(did)
}

pub fn landed(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let landing = parse(args)?;
    let top = &landing.checkout.top;
    let mut commits = Vec::new();
    for sha in &landing.shas {
        let commit = commit_id(top, sha)?;
        if !is_ancestor(top, &commit, "refs/remotes/origin/main")? {
            fail!("{sha} is not on origin/main in {}; `landed` records commits the push put there", top.display());
        }
        commits.push(commit);
    }
    let store = Store::resolve(&tool.places, &landing.args, top)?;
    let stamp = Stamp::resolve(tool.now, &landing.args, cwd)?;
    let repo = landing.checkout.repo();
    let did = store.with_write(&stamp, &format!("item landed {}", commits.join(" ")), |lists| {
        for named in &landing.items {
            if let Ok(item) = open_item(lists, named.id)
                && !item.running.is_empty()
            {
                running_in_repo(&item.running, &landing.checkout, named.id)?;
            }
        }
        record(lists, &landing.items, &repo, &commits, &stamp)
    })?;
    for line in did {
        note(line);
    }
    Ok(())
}

#[cfg(test)]
#[path = "landing_tests.rs"]
mod tests;

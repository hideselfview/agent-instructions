//! `builds item take`: an agent starts work on items, and `builds item park`:
//! an item keeps its work in a worktree with no agent while it waits in the
//! queue. The agent is the worktree's claim, read when asked and never
//! stored; the worktrees, branches and brief are checked before the store's
//! lock (git and the file system), what depends on the lists under it. Work
//! that spans repos names one worktree and branch per repo (`--worktree W
//! --branch B` repeated, the nth branch for the nth worktree); the items run
//! or park in all of them. Taking a parked item moves it to running in the
//! worktrees it is parked in.

use std::path::Path;
use std::process::Stdio;

use crate::checkout::{Checkout, git};
use crate::claim::Claim;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::Args;
use crate::lists::draft::require_brief_from;
use crate::lists::held::waits_on_open;
use crate::lists::item::open_item;
use crate::lists::item_command::blocking_words;
use crate::lists::parked::{self, Parked, parked_in};
use crate::lists::running::{Running, end, running_in, start};
use crate::lists::store::{self, Store};
use crate::lists::{Audience, Id, Lists, Stamp};
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "builds item take N [M...] --worktree W --branch B [--worktree W2 --branch B2] --brief P [--as design-session] [--held WHY]";
const PARK_USAGE: &str = "builds item park N --worktree W --branch B [--worktree W2 --branch B2]";

/// Does branch <branch> exist in the repo of <top>?
fn branch_exists(top: &Path, branch: &str) -> Result<bool> {
    let status = git(top)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context(|| format!("running git in {}", top.display()))?;
    Ok(status.success())
}

/// One worktree an item runs or parks in: checked, with its branch and the
/// claim that holds it, if any.
struct Place {
    top: String,
    checkout: Checkout,
    branch: String,
    claim: Option<Claim>,
}

/// The worktrees and branches <args> names, each a side worktree of its own
/// repo with the branch; <usage> is the command's.
fn places_named(args: &Args, usage: &str) -> Result<Vec<Place>> {
    let worktrees = args.values("worktree");
    let branches: Vec<String> = args
        .values("branch")
        .into_iter()
        .map(str::to_string)
        .collect();
    if worktrees.is_empty() {
        fail!("--worktree is required: {usage}");
    }
    if worktrees.len() != branches.len() {
        fail!(
            "{} --worktree and {} --branch given; each worktree takes the branch of its work, in the same order",
            worktrees.len(),
            branches.len()
        );
    }
    let mut places: Vec<Place> = Vec::new();
    for (worktree, branch) in worktrees.into_iter().zip(branches) {
        let checkout = Checkout::find(worktree)?;
        if !checkout.is_side() {
            fail!(
                "{} is not one of the repo's side worktrees (<repo>-worktrees/side-N); items run in those",
                checkout.top.display()
            );
        }
        if let Some(same) = places
            .iter()
            .find(|place| place.checkout.repo() == checkout.repo())
        {
            fail!(
                "{} and {} are both worktrees of {}; an item runs in one worktree per repo",
                same.checkout.top.display(),
                checkout.top.display(),
                checkout.repo()
            );
        }
        if !branch_exists(&checkout.top, &branch)? {
            fail!("{} has no branch {branch}", checkout.top.display());
        }
        places.push(Place {
            top: checkout.top.display().to_string(),
            claim: Claim::read(&checkout.claim_file())?,
            checkout,
            branch,
        });
    }
    Ok(places)
}

/// Do <places> name the same worktrees and branches as <held>, in any order?
fn same_places<'a>(
    places: &[Place],
    held: impl Iterator<Item = (&'a String, &'a String)> + Clone,
) -> bool {
    places.len() == held.clone().count()
        && places.iter().all(|place| {
            held.clone()
                .any(|(worktree, branch)| *worktree == place.top && *branch == place.branch)
        })
}

/// The worktrees and branches of <held>, as a refusal lists them.
fn places_words<'a>(held: impl Iterator<Item = (&'a String, &'a String)>) -> String {
    held.map(|(worktree, branch)| format!("{worktree} on {branch}"))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// Refuses <places> that a repo of <lists> does not hold, or whose worktree
/// holds another branch of work or the parked work of an item not in <ids>
/// (the items that move there).
fn require_places_free(lists: &Lists, places: &[Place], ids: &[Id]) -> Result<()> {
    for Place {
        top,
        checkout,
        branch,
        ..
    } in places
    {
        if !lists.repos.contains(&checkout.repo()) {
            fail!(
                "{top} is a worktree of repo {}, which is not one of project {}'s ({})",
                checkout.repo(),
                lists.project,
                lists.repos.join(", ")
            );
        }
        if let Some(other) = running_in(lists, top).into_iter().find(|i| {
            i.running
                .iter()
                .any(|r| r.worktree == *top && r.branch != *branch)
        }) {
            let running = other
                .running
                .iter()
                .find(|r| r.worktree == *top)
                .expect("filtered");
            fail!(
                "{top} already runs {} on branch {}, not {branch}: a worktree holds one branch of work",
                other.id,
                running.branch
            );
        }
        if let Some(other) = parked_in(lists, top).filter(|other| !ids.contains(&other.id)) {
            fail!(
                "{top} holds {} \"{}\" parked: take that item there, or work elsewhere",
                other.id,
                other.title
            );
        }
    }
    Ok(())
}

pub fn take(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["worktree", "branch", "brief", "as", "held"], &[])?;
    if args.positional().is_empty() {
        fail!("usage: {USAGE}");
    }
    let ids: Vec<Id> = args
        .positional()
        .iter()
        .map(|id| Id::parse(id))
        .collect::<Result<_>>()?;
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
    let brief = args.require("brief")?.to_string();
    let held = args.value("held")?.map(str::to_string);
    if let Some(why) = &held {
        if why.trim().is_empty() {
            fail!("--held needs the words that say what the work waits on");
        }
        crate::claim::require_one_line("--held", why)?;
    }
    let places = places_named(&args, USAGE)?;
    let mut agents = Vec::new();
    for place in &places {
        match &place.claim {
            Some(claim) if !claim.who.is_empty() => agents.push(claim.who.clone()),
            _ => fail!(
                "{} has no claim, and the claim is what names the agent: `builds claim {} WHO` first",
                place.top,
                place.top
            ),
        }
    }
    require_brief_from(cwd, &brief)?;

    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("item take {}", ids.iter().map(|i| i.0.to_string()).collect::<Vec<_>>().join(" ")), |db| {
        let lists = &store::read(db)?;
        require_places_free(lists, &places, &ids)?;
        for id in &ids {
            let item = open_item(lists, *id)?;
            if let Some(running) = item.running.first() {
                fail!("{id} already runs in {} on {}; `builds item stop {} --why WHY` first", running.worktree, running.branch, id.0);
            }
            let parked = item.parked.iter().map(|p| (&p.worktree, &p.branch));
            if !item.parked.is_empty() && !same_places(&places, parked.clone()) {
                fail!("{id} is parked in {}; take it there, where its work is", places_words(parked));
            }
            match (item.r#for, design_session) {
                (Audience::DesignSession, false) => fail!("{id} is the design session's; take it with --as design-session"),
                (Audience::Coordinator, true) => fail!("{id} is the coordinator's; --as design-session takes only the design session's items"),
                _ => {}
            }
            if held.is_some() {
                if !waits_on_open(lists, item) {
                    fail!("{id} waits on nothing open, so there is nothing to hold it for; take it without --held");
                }
                continue;
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
        for id in &ids {
            parked::end(db, *id)?;
        }
        start(db, &ids, &running)?;
        // Last, so a refusal above leaves the claims as they were.
        if let Some(why) = &held {
            for place in &places {
                let file = place.checkout.claim_file();
                let claim = Claim::read(&file)?.expect("read above");
                Claim {
                    note: Some(why.clone()),
                    note_state: Some("held".into()),
                    ..claim
                }
                .write(&file)?;
            }
        }
        Ok(())
    })?;
    let named: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    let wheres: Vec<String> = places
        .iter()
        .zip(&agents)
        .map(|(p, agent)| format!("{} on {} for {agent}", p.top, p.branch))
        .collect();
    note(format!(
        "{} run in {}",
        named.join(", "),
        wheres.join(" and ")
    ));
    Ok(())
}

/// `builds item park N --worktree W --branch B`: open item N keeps its work in
/// each worktree W on its branch B with no agent, and waits in the queue
/// (`builds next` names the worktree) until an agent takes it there. A
/// running item must park where it runs, and stops running. A worktree that
/// another item runs or is parked in is refused. The store's write comes
/// first, then each worktree's claim goes: no agent works there.
pub fn park(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["worktree", "branch"], &[])?;
    let [id] = args.exactly(1, PARK_USAGE)? else {
        unreachable!("exactly one")
    };
    let id = Id::parse(id)?;
    let places = places_named(&args, PARK_USAGE)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("item park {id}"), |db| {
        let lists = &store::read(db)?;
        require_places_free(lists, &places, &[id])?;
        let item = open_item(lists, id)?;
        if !item.parked.is_empty() {
            fail!(
                "{id} is parked already, in {}",
                places_words(item.parked.iter().map(|p| (&p.worktree, &p.branch)))
            );
        }
        let running = item.running.iter().map(|r| (&r.worktree, &r.branch));
        if !item.running.is_empty() && !same_places(&places, running.clone()) {
            fail!(
                "{id} runs in {}; park it there, where its work is",
                places_words(running)
            );
        }
        for place in &places {
            if let Some(other) = running_in(lists, &place.top)
                .into_iter()
                .find(|other| other.id != id)
            {
                fail!(
                    "{} still runs {} \"{}\", whose agent works there; park {id} in another worktree",
                    place.top,
                    other.id,
                    other.title
                );
            }
        }
        end(db, id)?;
        let parked: Vec<Parked> = places
            .iter()
            .map(|place| Parked {
                worktree: place.top.clone(),
                branch: place.branch.clone(),
                since: stamp.clone(),
            })
            .collect();
        parked::park(db, id, &parked)
    })?;
    for place in &places {
        if let Some(claim) = crate::claim::unclaim(&place.checkout)? {
            note(format!("released {} (was {})", place.top, claim.text()));
        }
    }
    note(format!(
        "{id} is parked in {}; it waits in the queue",
        places_words(places.iter().map(|p| (&p.top, &p.branch)))
    ));
    Ok(())
}

//! `builds item take`: an agent starts work on items. The worktree's claim
//! names the agent; the command reads the claim
//! when it needs the agent and never stores it. The command checks the worktrees, branches and brief in git and
//! the file system before it takes the store's lock, and checks what
//! depends on the lists after it takes the lock. Work
//! that spans repos names one worktree and branch per repo (`--worktree W
//! --branch B` repeated, the nth branch for the nth worktree); the items run
//! in all of them. An item that runs already takes a worktree of each repo
//! of its project that it does not run in yet: its work grew into another
//! repo, and the take adds that worktree to the run that goes on, with the
//! brief the item runs with. An item that keeps worktrees (`builds item stop`
//! records them when its run ends with work left there) is taken in them,
//! without --worktree and --branch, and nowhere else.
//!
//! `--repo R --branch B` in place of `--worktree W --branch B` hands out a
//! worktree of repo R (`worktree::hand_out`, which `builds new` uses) on the
//! new branch B, and a take that then fails gives each worktree back
//! (`Handed::undo`), so a refused take leaves no claim.
//!
//! Before the take's write, take refuses a brief that pastes a coding rule or
//! workflow text (pasted.rs). Then it checks the brief against the product spec of each named worktree's repo that has
//! spec files (spec_check.rs) and records each check in its own write. Take
//! prints each behavior the brief states otherwise than the spec, or that the
//! spec doesn't state, as a spec edit for the agent's commit, and takes the
//! items. `--reviewer` and `--rules-root` name the reviewer program and the
//! rules checkout the spec check uses in place of the tool's.

use std::path::Path;

use crate::args::Args;
use crate::checkout::Checkout;
use crate::claim::{Claim, Note};
use crate::fail;
use crate::fail::{Fail, Result};
use crate::git::branch_exists;
use crate::host::Ssh;
use crate::lists::draft::find_brief;
use crate::lists::held::waits_on_open;
use crate::lists::item::{Item, open_item};
use crate::lists::item_command::blocking_words;
use crate::lists::kept_work::{self, kept_in, places_words};
use crate::lists::running::{
    PlaceTable, Running, SpanKind, insert_places, open_span, place_in_repo, running_in, start,
};
use crate::lists::spec_check::{self, Brief, Setup};
use crate::lists::store::{self, Store};
use crate::lists::{Audience, Id, Lists, Stamp};
use crate::model_tier::Tier;
use crate::output::note;
use crate::review::Reviewer;
use crate::tool::Tool;
use crate::worktree::{Handed, Start, hand_out};

/// One worktree an item runs in: checked, with its branch and the
/// claim that holds it, if any.
#[derive(Clone)]
struct Place {
    top: String,
    checkout: Checkout,
    branch: String,
    claim: Option<Claim>,
}

impl Place {
    /// The worktree's claim as the file holds it now: `builds release` or
    /// health may have removed it since take first read it.
    fn claim_now(&self) -> Result<Claim> {
        Claim::read(&self.checkout.claim_file())?.ok_or_else(|| {
            crate::fail::Fail::Message(format!(
                "{} lost its claim while take ran; run `builds claim {} WHO` and take again",
                self.top, self.top
            ))
        })
    }
}

/// The worktrees and branches <args> names, each a side worktree of its own
/// repo with the branch.
fn places_named(args: &Args) -> Result<Vec<Place>> {
    let worktrees: Vec<String> = args
        .values("worktree")
        .into_iter()
        .map(str::to_string)
        .collect();
    let branches: Vec<String> = args
        .values("branch")
        .into_iter()
        .map(str::to_string)
        .collect();
    places_of(worktrees, branches)
}

/// The side worktree <worktree> names, refused when it is not a side worktree
/// or <earlier>, the worktrees named before it, hold another worktree of its
/// repo: an item runs in one worktree per repo.
fn side_checkout<'a>(
    worktree: &str,
    mut earlier: impl Iterator<Item = &'a Checkout>,
) -> Result<Checkout> {
    let checkout = Checkout::find(worktree)?;
    if !checkout.is_side() {
        fail!(
            "{} is not one of the repo's side worktrees (<repo>-worktrees/side-N); items run in those",
            checkout.top.display()
        );
    }
    if let Some(same) = earlier.find(|other| other.repo() == checkout.repo()) {
        fail!(
            "{} and {} are both worktrees of {}; an item runs in one worktree per repo",
            same.top.display(),
            checkout.top.display(),
            checkout.repo()
        );
    }
    Ok(checkout)
}

/// Refuses <checkout> when a repo of <lists> does not hold it.
fn require_project_repo(lists: &Lists, checkout: &Checkout) -> Result<()> {
    if !lists.repos.contains(&checkout.repo()) {
        fail!(
            "{} is a worktree of repo {}, which is not one of project {}'s ({})",
            checkout.top.display(),
            checkout.repo(),
            lists.project,
            lists.repos.join(", ")
        );
    }
    Ok(())
}

/// The places of <worktrees> and their <branches>, checked as
/// `places_named` checks them.
fn places_of(worktrees: Vec<String>, branches: Vec<String>) -> Result<Vec<Place>> {
    if worktrees.len() != branches.len() {
        fail!(
            "{} --worktree and {} --branch given; each worktree takes the branch of its work, in the same order",
            worktrees.len(),
            branches.len()
        );
    }
    let mut places: Vec<Place> = Vec::new();
    for (worktree, branch) in worktrees.into_iter().zip(branches) {
        let checkout = side_checkout(&worktree, places.iter().map(|place| &place.checkout))?;
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

/// Refuses <places> that a repo of <lists> does not hold, or whose worktree
/// holds another branch of work or the kept work of an item not in <ids>
/// (the items that move there).
fn require_places_free(lists: &Lists, places: &[Place], ids: &[Id]) -> Result<()> {
    for Place {
        top,
        checkout,
        branch,
        ..
    } in places
    {
        require_project_repo(lists, checkout)?;
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
        if let Some(other) = kept_in(lists, top).filter(|other| !ids.contains(&other.id)) {
            fail!(
                "{top} holds the kept work of {} \"{}\": take that item there, or work elsewhere",
                other.id,
                other.title
            );
        }
    }
    Ok(())
}

/// What a take does with its items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Take {
    /// The items do not run: they start running in the places.
    Start,
    /// The items run already: each place joins the run that goes on, in a
    /// repo the items do not run in yet.
    Join,
}

/// Refuses <ids> that cannot run in <places> with <brief>: a running item
/// that runs in one of <places>' repos already or runs with another brief,
/// items of which some run and some do not, one that keeps work elsewhere,
/// an item meant for the other audience than <design_session> names, and
/// one that waits on something not met, or, when <held>, one that waits on
/// nothing open, and, when it names a <tier>, one that runs at another. Says
/// whether the take starts the items or adds <places> to their runs.
fn require_takeable(
    lists: &Lists,
    places: &[Place],
    ids: &[Id],
    brief: &str,
    tier: Option<Tier>,
    design_session: bool,
    held: bool,
) -> Result<Take> {
    require_places_free(lists, places, ids)?;
    let mut take: Option<Take> = None;
    for id in ids {
        let item = open_item(lists, *id)?;
        let this = if item.running.is_empty() {
            Take::Start
        } else {
            Take::Join
        };
        match take {
            Some(first) if first != this => fail!(
                "some of the items run and some do not; take the running ones apart from the others"
            ),
            _ => take = Some(this),
        }
        if this == Take::Join {
            require_joinable(item, places, brief, tier)?;
        }
        let kept = item.kept.iter().map(|p| (&p.worktree, &p.branch));
        if !item.kept.is_empty() && !same_places(places, kept.clone()) {
            fail!(
                "{id} keeps its work in {}; take it there",
                places_words(kept)
            );
        }
        if item.backlog.is_some() {
            fail!(
                "{id} is in the backlog: only the person starts it (`builds item start {} --said WORDS`)",
                id.0
            );
        }
        match (item.r#for, design_session) {
            (Audience::DesignSession, false) => {
                fail!("{id} is the design session's; take it with --as design-session")
            }
            (Audience::Coordinator, true) => fail!(
                "{id} is the coordinator's; --as design-session takes only the design session's items"
            ),
            _ => {}
        }
        if held {
            if !waits_on_open(lists, item) {
                fail!(
                    "{id} waits on nothing open, so there is nothing to hold it for; take it without --held"
                );
            }
            continue;
        }
        let blocking = blocking_words(lists, item);
        if !blocking.is_empty() {
            fail!(
                "{id} waits on {}; take it once those are done",
                blocking.join("; ")
            );
        }
    }
    Ok(take.expect("a take names an item"))
}

/// Refuses running <item> in <places> unless each is in a repo the item
/// does not run in yet and <brief> is the brief it runs with: an item runs
/// in one worktree per repo, with one brief.
fn require_joinable(item: &Item, places: &[Place], brief: &str, tier: Option<Tier>) -> Result<()> {
    let id = item.id;
    for place in places {
        let repo = place.checkout.repo();
        if let Some(running) = place_in_repo(&item.running, &repo) {
            fail!(
                "{id} already runs in {} on {}, its worktree of repo {repo}; an item runs in one worktree per repo, so stop it first (`builds item stop {} --why WHY`) to run it elsewhere",
                running.worktree,
                running.branch,
                id.0
            );
        }
    }
    let first = item.running.first().expect("a running item");
    if let Some(tier) = tier.filter(|tier| *tier != first.tier) {
        fail!(
            "{id} runs at the {} tier; --tier {} would change it, and an item keeps the tier it was taken at",
            first.tier.word(),
            tier.word()
        );
    }
    let runs_with = &first.brief;
    if runs_with != brief {
        fail!(
            "{id} runs with brief {runs_with}, and a worktree that joins its run takes that brief; name it with --brief, or give the item a new brief with `builds item handoff {} --brief P` first",
            id.0
        );
    }
    Ok(())
}

/// The reviewer program and rules checkout the take's checks of the brief run
/// with: `--reviewer` and `--rules-root`, else the tool's.
fn reviewer_of(tool: &Tool, args: &Args) -> Result<Reviewer> {
    Ok(Reviewer {
        program: tool.reviewer.program_for(args.value("reviewer")?),
        rules_root: match args.value("rules-root")? {
            Some(root) => std::path::PathBuf::from(root),
            None => tool.reviewer.rules_root.clone(),
        },
    })
}

/// Where a take records its spec checks: the store, the stamp of the write,
/// and the items the checks are for. A check of a brief that takes nothing
/// records nothing.
struct Recording<'a> {
    store: &'a Store,
    stamp: &'a Stamp,
    ids: &'a [Id],
}

/// Checks <brief> against the product spec of each of <checkouts>' repos that
/// has spec files in <lists>, records each check, and prints what the checks
/// found. Fails when a check gave no usable answer.
fn check_spec(
    recording: Option<&Recording>,
    lists: &Lists,
    checkouts: &[Checkout],
    brief: &Brief,
    reviewer: &Reviewer,
) -> Result<()> {
    let worktrees: Vec<(String, std::path::PathBuf)> = checkouts
        .iter()
        .map(|checkout| (checkout.repo(), checkout.top.clone()))
        .collect();
    let targets = spec_check::targets(lists, &worktrees)?;
    if targets.is_empty() {
        let repos: Vec<&str> = worktrees.iter().map(|(repo, _)| repo.as_str()).collect();
        note(format!(
            "no spec check: project {} names no spec files for {}",
            lists.project,
            repos.join(" or ")
        ));
        return Ok(());
    }
    let setup = Setup::load(&reviewer.rules_root)?;
    let checked = spec_check::check_all(&reviewer.program, &setup, brief, &targets);
    let listed = match recording {
        Some(Recording { store, stamp, ids }) => {
            let check_ids = store.write(stamp, &format!("spec check of {}", brief.name), |db| {
                spec_check::record(db, stamp, brief, ids, &checked)
            })?;
            let shown: Vec<String> = check_ids.iter().map(|id| id.to_string()).collect();
            let first = ids.first().expect("a take names an item");
            format!(
                "\nbuilds item show {} lists spec check {}",
                first.0,
                shown.join(", ")
            )
        }
        None => String::new(),
    };
    if let Some(failures) = spec_check::failures(&checked) {
        fail!("{failures}{listed}");
    }
    for line in spec_check::notes(&checked) {
        note(line);
    }
    Ok(())
}

/// The checks of a brief that `item take` and `item check-brief` share: the
/// pasted-text check and the spec check of each of
/// <checkouts>' repos. Records the spec checks when <recording> is given.
fn check_brief(
    recording: Option<&Recording>,
    lists: &Lists,
    checkouts: &[Checkout],
    brief: &Brief,
    reviewer: &Reviewer,
) -> Result<()> {
    crate::lists::pasted::require_unpasted(brief, &reviewer.rules_root, &lists.project)?;
    check_spec(recording, lists, checkouts, brief, reviewer)
}

/// `builds item check-brief`: runs the checks `item take` runs on a brief
/// (pasted text, spec) in the worktrees <args> names, and takes and
/// records nothing.
pub fn check(tool: &Tool, args: &Args, cwd: &Path) -> Result<()> {
    let brief = args.require("brief")?.to_string();
    let worktrees = args.values("worktree");
    if worktrees.is_empty() {
        fail!(
            "check-brief needs --worktree: the spec check reads each worktree's repo: {usage}",
            usage = crate::command::Command::Item(crate::command::item::CheckBrief).usage_text(),
        );
    }
    let mut checkouts: Vec<Checkout> = Vec::new();
    for worktree in worktrees {
        let checkout = side_checkout(worktree, checkouts.iter())?;
        checkouts.push(checkout);
    }
    let store = Store::resolve(&tool.places, args, cwd)?;
    let brief_path = find_brief(&tool.places, &store.repos()?, &brief)?;
    note(format!("brief {brief} is {}", brief_path.display()));
    let read_brief = Brief::read(&brief, &brief_path)?;
    let lists = store.load()?;
    for checkout in &checkouts {
        require_project_repo(&lists, checkout)?;
    }
    let reviewer = reviewer_of(tool, args)?;
    check_brief(None, &lists, &checkouts, &read_brief, &reviewer)?;
    note(format!(
        "brief {brief} passes the checks of `builds item take`; nothing was taken or recorded"
    ));
    Ok(())
}

pub fn take(tool: &Tool, ssh: &Ssh, args: &Args, cwd: &Path) -> Result<()> {
    let ids = args.positional_ids()?;
    let design_session = match args.value("as")? {
        None => false,
        Some("design-session") => true,
        Some(other) => fail!("--as takes design-session, not '{other}'"),
    };
    let tier = match args.value("tier")? {
        None => None,
        Some(word) => match Tier::from_word(word) {
            Some(tier) => Some(tier),
            None => fail!("--tier takes implementation or strongest, not '{word}'"),
        },
    };
    let brief = args.require("brief")?.to_string();
    let held = args.value("held")?.map(str::to_string);
    if let Some(why) = &held {
        if why.trim().is_empty() {
            fail!("--held needs the words that say what the work waits on");
        }
        crate::claim::require_one_line("--held", why)?;
    }
    let store = Store::resolve(&tool.places, args, cwd)?;
    let stamp = Stamp::resolve(tool.now, args, cwd)?;
    let request = Request {
        tool,
        args,
        store: &store,
        stamp: &stamp,
        ids: &ids,
        brief: &brief,
        tier,
        design_session,
        held: held.as_deref(),
    };
    if args.values("repo").is_empty() {
        let places = if args.values("worktree").is_empty() {
            kept_places(&store.load()?, &ids)?
        } else {
            places_named(args)?
        };
        return request.run(&places);
    }
    if !args.values("worktree").is_empty() {
        fail!(
            "--repo hands out a worktree and --worktree names one; give one of them: {usage}",
            usage = crate::command::Command::Item(crate::command::item::Take).usage_text()
        );
    }
    let handed = hand_out_places(tool, ssh, &store.load()?, &stamp, args)?;
    let places: Vec<Place> = handed.iter().map(|(place, _)| place.clone()).collect();
    match request.run(&places) {
        Ok(()) => Ok(()),
        Err(error) => Err(give_back(tool, handed, error)),
    }
}

/// Undoes each of <handed>, last first, after <error>, and returns <error>
/// with the failure of any undo.
fn give_back(tool: &Tool, handed: Vec<(Place, Handed)>, error: Fail) -> Fail {
    handed
        .into_iter()
        .rev()
        .fold(error, |error, (_, made)| made.undo(tool, error))
}

/// What `take` runs with, once it has its worktrees.
struct Request<'a> {
    tool: &'a Tool,
    args: &'a Args,
    store: &'a Store,
    stamp: &'a Stamp,
    ids: &'a [Id],
    brief: &'a str,
    tier: Option<Tier>,
    design_session: bool,
    held: Option<&'a str>,
}

/// A worktree for each `--repo` of <args>, claimed for the agent <stamp>
/// names, on a new branch from origin/main named by the nth `--branch`, with
/// the `Handed` that undoes it. When one repo cannot give its worktree, the
/// worktrees of the repos before it go back as they were.
fn hand_out_places(
    tool: &Tool,
    ssh: &Ssh,
    lists: &Lists,
    stamp: &Stamp,
    args: &Args,
) -> Result<Vec<(Place, Handed)>> {
    let repos = args.values("repo");
    let branches = args.values("branch");
    if repos.len() != branches.len() {
        fail!(
            "{} --repo and {} --branch given; each repo takes the new branch of its work, in the same order",
            repos.len(),
            branches.len()
        );
    }
    let mains: Vec<std::path::PathBuf> = repos
        .iter()
        .map(|repo| {
            if !lists.repos.iter().any(|held| held == repo) {
                fail!(
                    "{repo} is not one of project {}'s repos ({})",
                    lists.project,
                    lists.repos.join(", ")
                );
            }
            Ok(tool.places.home.join(crate::host::clone_path(repo)))
        })
        .collect::<Result<_>>()?;
    for (i, repo) in repos.iter().enumerate() {
        if repos[..i].contains(repo) {
            fail!("--repo {repo} twice: an item runs in one worktree per repo");
        }
        if branch_exists(&mains[i], branches[i])? {
            fail!(
                "{repo} has the branch {} already; --worktree W --branch {} takes it in a worktree, or name a new branch",
                branches[i],
                branches[i]
            );
        }
    }
    let mut handed: Vec<(Place, Handed)> = Vec::new();
    for (main, branch) in mains.iter().zip(branches) {
        let start = Start::Fresh(Some(branch.to_string()));
        let made = hand_out(
            tool,
            ssh,
            &main.display().to_string(),
            start,
            &stamp.by,
            Some(&lists.project),
        );
        let placed = made.and_then(|made| match Claim::read(&made.checkout.claim_file()) {
            Ok(claim) => Ok((
                Place {
                    top: made.checkout.top.display().to_string(),
                    claim,
                    checkout: made.checkout.clone(),
                    branch: branch.to_string(),
                },
                made,
            )),
            Err(error) => Err(made.undo(tool, error)),
        });
        match placed {
            Ok(one) => handed.push(one),
            Err(error) => return Err(give_back(tool, handed, error)),
        }
    }
    Ok(handed)
}

impl Request<'_> {
    /// Takes the items in <places>, claimed worktrees with their branches.
    fn run(&self, places: &[Place]) -> Result<()> {
        let Request {
            tool,
            args,
            store,
            stamp,
            ids,
            brief,
            tier,
            design_session,
            held,
        } = *self;
        let held = held.map(str::to_string);
        let brief = brief.to_string();
        let mut agents = Vec::new();
        for place in places {
            match &place.claim {
                Some(claim) if !claim.who.is_empty() => agents.push(claim.who.clone()),
                _ => fail!(
                    "{} has no claim, and the claim is what names the agent: run `builds claim {} WHO` first",
                    place.top,
                    place.top
                ),
            }
        }
        let brief_path = find_brief(&tool.places, &store.repos()?, &brief)?;
        note(format!("brief {brief} is {}", brief_path.display()));
        let read_brief = Brief::read(&brief, &brief_path)?;
        // The spec check runs a reviewer for minutes, so take refuses what the
        // store refuses before the check, and again under the store's lock.
        let lists = store.load()?;
        require_takeable(
            &lists,
            places,
            ids,
            &brief,
            tier,
            design_session,
            held.is_some(),
        )?;
        let reviewer = reviewer_of(tool, args)?;
        let recording = Recording { store, stamp, ids };
        let checkouts: Vec<Checkout> = places.iter().map(|place| place.checkout.clone()).collect();
        check_brief(Some(&recording), &lists, &checkouts, &read_brief, &reviewer)?;
        let take = store.write(
            stamp,
            &format!(
                "item take {}",
                ids.iter()
                    .map(|i| i.0.to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            |db| {
                let lists = &store::read(db)?;
                let take = require_takeable(
                    lists,
                    places,
                    ids,
                    &brief,
                    tier,
                    design_session,
                    held.is_some(),
                )?;
                match take {
                    Take::Start => {
                        let running: Vec<Running> = places
                            .iter()
                            .map(|place| Running {
                                worktree: place.top.clone(),
                                branch: place.branch.clone(),
                                brief: brief.clone(),
                                since: stamp.clone(),
                                tier: tier.unwrap_or(Tier::Implementation),
                            })
                            .collect();
                        for id in ids {
                            kept_work::end(db, *id)?;
                        }
                        start(db, ids, &running)?;
                    }
                    Take::Join => {
                        // An item runs in one worktree per repo: the database
                        // refuses a second place in a repo where it runs.
                        for id in ids {
                            insert_places(
                                db,
                                PlaceTable::Running,
                                *id,
                                places
                                    .iter()
                                    .map(|place| (place.top.as_str(), place.branch.as_str())),
                            )?;
                        }
                    }
                }
                // Last, so a refusal above leaves the claims as they were. A
                // held take notes why the work waits, and any take ends a claim's
                // wait for its item.
                if held.is_some() {
                    for id in ids {
                        open_span(db, *id, SpanKind::Held, stamp.at)?;
                    }
                }
                for place in places {
                    let claim = place.claim_now()?;
                    let note = match &held {
                        Some(why) => Some(Note::Held(why.clone())),
                        None if claim.is_starting() => None,
                        None => continue,
                    };
                    Claim { note, ..claim }.write(&place.checkout.claim_file())?;
                }
                Ok(take)
            },
        )?;
        let named: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        let wheres: Vec<String> = places
            .iter()
            .zip(&agents)
            .map(|(p, agent)| format!("{} on {} for {agent}", p.top, p.branch))
            .collect();
        let also = match take {
            Take::Start => "",
            Take::Join => "also ",
        };
        note(format!(
            "{} {also}run in {}",
            named.join(", "),
            wheres.join(" and ")
        ));
        Ok(())
    }
}

/// The places of the items <ids> keep, for a take that names no worktree: the
/// items must all keep the same worktrees and branches, and the take runs
/// them there.
fn kept_places(lists: &Lists, ids: &[Id]) -> Result<Vec<Place>> {
    let mut kept: Option<Vec<(String, String)>> = None;
    for id in ids {
        let item = open_item(lists, *id)?;
        if item.kept.is_empty() {
            fail!(
                "{id} keeps no worktree, so --worktree and --branch are required: {usage}",
                usage = crate::command::Command::Item(crate::command::item::Take).usage_text()
            );
        }
        let places: Vec<(String, String)> = item
            .kept
            .iter()
            .map(|k| (k.worktree.clone(), k.branch.clone()))
            .collect();
        match &kept {
            Some(first) if *first != places => {
                fail!("{id} keeps other worktrees than the items before it; take them apart")
            }
            _ => kept = Some(places),
        }
    }
    let (worktrees, branches) = kept.expect("a take names an item").into_iter().unzip();
    places_of(worktrees, branches)
}

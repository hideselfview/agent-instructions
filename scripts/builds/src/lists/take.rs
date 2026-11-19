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
//! Before the take's write, take checks that each sentence of the brief that
//! states a ruling of the person's or a standing prohibition cites the
//! decision or rule behind it (rulings.rs), and refuses the take when one
//! cites nothing or names what builds does not keep. Then it checks the
//! brief against the product spec of each named worktree's repo that has
//! spec files (spec_check.rs) and records each check in its own write. A
//! conflict refuses the take unless `--spec-decided` names the decision or
//! question that records the person's choice on it; take prints each
//! behavior the spec doesn't state and takes the items. `--reviewer` and
//! `--rules-root` name the reviewer program and the rules checkout both
//! checks use in place of the tool's.

use std::path::Path;

use crate::checkout::{Checkout, branch_exists};
use crate::claim::{Claim, Note};
use crate::fail;
use crate::fail::Result;
use crate::lists::args::Args;
use crate::lists::draft::brief_file;
use crate::lists::held::waits_on_open;
use crate::lists::item::{Item, open_item};
use crate::lists::item_command::blocking_words;
use crate::lists::kept_work::{self, kept_in, places_words};
use crate::lists::rulings::{self, Source, Text};
use crate::lists::running::{
    PlaceTable, Running, SpanKind, insert_places, open_span, place_in_repo, running_in, start,
};
use crate::lists::spec_check::{self, Brief, Setup};
use crate::lists::store::{self, Store};
use crate::lists::{Audience, Id, Kind, Lists, Stamp};
use crate::review::Reviewer;
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "builds item take N [M...] [--worktree W --branch B [--worktree W2 --branch B2]] --brief P [--as design-session] [--held WHY] [--spec-decided N] [--rules-root DIR] [--reviewer PROGRAM]";

/// One worktree an item runs in: checked, with its branch and the
/// claim that holds it, if any.
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
        let checkout = Checkout::find(&worktree)?;
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
/// nothing open. Says whether the take starts the items or adds <places> to
/// their runs.
fn require_takeable(
    lists: &Lists,
    places: &[Place],
    ids: &[Id],
    brief: &str,
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
            require_joinable(item, places, brief)?;
        }
        let kept = item.kept.iter().map(|p| (&p.worktree, &p.branch));
        if !item.kept.is_empty() && !same_places(places, kept.clone()) {
            fail!(
                "{id} keeps its work in {}; take it there",
                places_words(kept)
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
fn require_joinable(item: &Item, places: &[Place], brief: &str) -> Result<()> {
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
    let runs_with = &item.running.first().expect("a running item").brief;
    if runs_with != brief {
        fail!(
            "{id} runs with brief {runs_with}, and a worktree that joins its run takes that brief; name it with --brief, or give the item a new brief with `builds item handoff {} --brief P` first",
            id.0
        );
    }
    Ok(())
}

/// What the take's checks of the brief run with, from its flags: the
/// reviewer program and rules checkout (`--reviewer`, `--rules-root`, else
/// the tool's), and the record of the person's choice on a spec conflict
/// (`--spec-decided`).
struct Checks {
    reviewer: Reviewer,
    decided: Option<(Id, Kind)>,
}

impl Checks {
    fn of(tool: &Tool, lists: &Lists, args: &Args) -> Result<Checks> {
        Ok(Checks {
            reviewer: Reviewer {
                program: match args.value("reviewer")? {
                    Some(program) => std::path::PathBuf::from(program),
                    None => tool.reviewer.program.clone(),
                },
                rules_root: match args.value("rules-root")? {
                    Some(root) => std::path::PathBuf::from(root),
                    None => tool.reviewer.rules_root.clone(),
                },
            },
            decided: match args.value("spec-decided")? {
                Some(text) => Some(spec_check::decided_ref(lists, text)?),
                None => None,
            },
        })
    }
}

/// Checks <brief> against the product spec of each of <places>' repos that
/// has spec files in <lists>, records each check, and fails the take when a
/// check failed or found a conflict that `--spec-decided` does not settle.
/// Prints what the checks found that does not stop the take.
fn check_spec(
    store: &Store,
    stamp: &Stamp,
    lists: &Lists,
    places: &[Place],
    ids: &[Id],
    brief: &Brief,
    checks: &Checks,
) -> Result<()> {
    let (reviewer, decided) = (&checks.reviewer, checks.decided);
    let worktrees: Vec<(String, std::path::PathBuf)> = places
        .iter()
        .map(|place| (place.checkout.repo(), place.checkout.top.clone()))
        .collect();
    let targets = spec_check::targets(lists, &worktrees)?;
    if targets.is_empty() {
        let repos: Vec<&str> = worktrees.iter().map(|(repo, _)| repo.as_str()).collect();
        if decided.is_some() {
            fail!(
                "--spec-decided names the person's choice on a spec conflict, and project {} names no spec files for {}, so no spec check runs",
                lists.project,
                repos.join(" or ")
            );
        }
        note(format!(
            "no spec check: project {} names no spec files for {}",
            lists.project,
            repos.join(" or ")
        ));
        return Ok(());
    }
    let setup = Setup::load(&reviewer.rules_root)?;
    let checked = spec_check::check_all(&reviewer.program, &setup, brief, &targets);
    let check_ids = store.write(stamp, &format!("spec check of {}", brief.name), |db| {
        spec_check::record(db, stamp, brief, ids, &checked, decided)
    })?;
    let shown: Vec<String> = check_ids.iter().map(|id| id.to_string()).collect();
    let first = ids.first().expect("a take names an item");
    let listed = format!(
        "builds item show {} lists spec check {}",
        first.0,
        shown.join(", ")
    );
    if let Some(failures) = spec_check::failures(&checked) {
        fail!("{failures}\n{listed}");
    }
    for line in spec_check::notes(&checked) {
        note(line);
    }
    if let Some(conflicts) = spec_check::conflicts(brief, &checked) {
        match decided {
            None => fail!(
                "{conflicts}\nThe person decides each conflict between a brief and the spec, so builds took nothing. Ask the person about each conflict, and say which text looks wrong and why: the spec is a draft that the coding rules outrank. Once they decide, record the choice as a decision or question, then take the items again with --spec-decided N, or change the brief when the brief is wrong.\n{listed}"
            ),
            Some((id, _)) => note(format!(
                "{conflicts}the person decided these conflicts in {id}, so the take goes on"
            )),
        }
    }
    Ok(())
}

pub fn take(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(
        args,
        &[
            "worktree",
            "branch",
            "brief",
            "as",
            "held",
            "spec-decided",
            "rules-root",
            "reviewer",
        ],
        &[],
    )?;
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
            fail!("the command names {id} twice");
        }
    }
    let design_session = match args.value("as")? {
        None => false,
        Some("design-session") => true,
        Some(other) => fail!("--as takes design-session, not '{other}'"),
    };
    let brief = args.require("brief")?.to_string();
    let held = args.value("held")?.map(str::to_string);
    if let Some(why) = &held {
        if why.trim().is_empty() {
            fail!("--held needs the words that say what the work waits on");
        }
        crate::claim::require_one_line("--held", why)?;
    }
    let places = if args.values("worktree").is_empty() {
        kept_places(&Store::resolve(&tool.places, &args, cwd)?.load()?, &ids)?
    } else {
        places_named(&args)?
    };
    let mut agents = Vec::new();
    for place in &places {
        match &place.claim {
            Some(claim) if !claim.who.is_empty() => agents.push(claim.who.clone()),
            _ => fail!(
                "{} has no claim, and the claim is what names the agent: run `builds claim {} WHO` first",
                place.top,
                place.top
            ),
        }
    }
    let read_brief = Brief::read(&brief, &brief_file(cwd, &brief)?)?;

    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    // The spec check runs a reviewer for minutes, so take refuses what the
    // store refuses before the check, and again under the store's lock.
    let lists = store.load()?;
    require_takeable(
        &lists,
        &places,
        &ids,
        &brief,
        design_session,
        held.is_some(),
    )?;
    let checks = Checks::of(tool, &lists, &args)?;
    for line in rulings::require_cited(
        &checks.reviewer,
        &lists,
        Source::Brief,
        &[Text::new(
            format!("the brief {}", read_brief.name),
            &read_brief.text,
        )],
    )? {
        note(line);
    }
    check_spec(&store, &stamp, &lists, &places, &ids, &read_brief, &checks)?;
    let take = store.write(
        &stamp,
        &format!(
            "item take {}",
            ids.iter()
                .map(|i| i.0.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ),
        |db| {
            let lists = &store::read(db)?;
            let take =
                require_takeable(lists, &places, &ids, &brief, design_session, held.is_some())?;
            match take {
                Take::Start => {
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
                        kept_work::end(db, *id)?;
                    }
                    start(db, &ids, &running)?;
                }
                Take::Join => {
                    // An item runs in one worktree per repo: the database
                    // refuses a second place in a repo where it runs.
                    for id in &ids {
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
                for id in &ids {
                    open_span(db, *id, SpanKind::Held, stamp.at)?;
                }
            }
            for place in &places {
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

/// The places of the items <ids> keep, for a take that names no worktree: the
/// items must all keep the same worktrees and branches, and the take runs
/// them there.
fn kept_places(lists: &Lists, ids: &[Id]) -> Result<Vec<Place>> {
    let mut kept: Option<Vec<(String, String)>> = None;
    for id in ids {
        let item = open_item(lists, *id)?;
        if item.kept.is_empty() {
            fail!("{id} keeps no worktree, so --worktree and --branch are required: {USAGE}");
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

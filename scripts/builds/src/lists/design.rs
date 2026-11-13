//! Design changes: a product design change (a board or a design-system
//! component) recorded when its boards are drawn, with every design place it
//! changes resolved to the app work that follows or to how it was checked
//! that none does, and waiting on the person's review until it can land.
//! These are the operations on the state a write holds under the lock;
//! `design_command` is the commands around them, `design_paths` reads git.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use serde::Deserialize;

use crate::fail;
use crate::fail::Result;
use crate::lists::derive::{self, ItemState};
use crate::lists::design_paths::{Changes, PlaceKind};
use crate::lists::draft::{ItemDraft, add_item};
use crate::lists::{
    Board, DesignChange, DesignLanding, How, Id, Kind, Lists, Located, Record, Resolution, ReviewSection, Stamp, Standing,
    Wait,
};
use crate::text::local_minute;

/// How one changed place is resolved in a request.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaceAnswer {
    /// An open item that does the app work for it.
    Item(Id),
    /// A new item for the app work; it waits on this design change.
    New(ItemDraft),
    /// How it was checked that the app already matches.
    Matches(String),
}

/// A design change as `design record` and `design amend` take it: `title`
/// and `details` are required to record and optional to amend.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignRequest {
    pub title: Option<String>,
    pub details: Option<String>,
    /// A worktree of the repo the commits are in.
    pub worktree: String,
    /// The commits, e.g. `origin/main..HEAD`.
    pub range: String,
    /// For every design place the range changes, and for each component a
    /// changed shared source changes the drawing of.
    #[serde(default)]
    pub places: BTreeMap<String, PlaceAnswer>,
    /// Each board place's canvas link.
    #[serde(default)]
    pub boards: BTreeMap<String, Board>,
    /// How the review page reads: headings, the words under each, and its
    /// boards, which the dashboard renders live from the worktree. Required
    /// to record; an amend that gives it replaces the page, one that doesn't
    /// keeps it. Every board place of the change must be on it.
    pub review: Option<Vec<ReviewSection>>,
    /// Why shared design files changed and no drawing did.
    pub no_app_place: Option<String>,
}

/// What a record or amend made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub design: Id,
    pub new_items: Vec<Id>,
}

fn require_text(what: &str, text: Option<&String>) -> Result<String> {
    match text {
        Some(text) if !text.trim().is_empty() => Ok(text.clone()),
        Some(_) => fail!("{what} is empty"),
        None => fail!("a design change needs its {what}"),
    }
}

fn open_item(lists: &Lists, id: Id) -> Result<()> {
    match lists.locate(id) {
        None => fail!("place resolved by item {id}, which no record has"),
        Some(Located { kind, .. }) if kind != Kind::Item => {
            fail!("place resolved by item {id}, but it is a {}", kind.name())
        }
        Some(Located { standing: Standing::Closed(closed), .. }) => {
            fail!("place resolved by item {id}, which is closed ({})", closed.how.label())
        }
        Some(_) => Ok(()),
    }
}

/// Resolves <changes> in design change <id> by <request>: each answer is
/// applied (a new item waits on the change), then every changed place must
/// be resolved and the shared files and sources answered for. <affected> are
/// the request's places that name existing components rather than something
/// the range changes. Refuses with every unresolved place named.
fn resolve(
    lists: &mut Lists,
    id: Id,
    request: &DesignRequest,
    worktree: &str,
    changes: &Changes,
    affected: &BTreeSet<String>,
    stamp: &Stamp,
) -> Result<Vec<Id>> {
    if !changes.is_design_change() {
        fail!("the range {} changes no design files", request.range);
    }
    let changed = changes.resolvable();
    let stray: Vec<&str> = request
        .places
        .keys()
        .filter(|path| !changed.contains(*path) && !affected.contains(*path))
        .map(String::as_str)
        .collect();
    if !stray.is_empty() {
        fail!(
            "`places` names {}, which the range does not change{}",
            stray.join(", "),
            if changes.sources.is_empty() {
                ""
            } else {
                " and which is no component folder of the design system; a changed shared source takes the components it changes the drawing of"
            }
        );
    }
    if !affected.is_empty() && changes.sources.is_empty() {
        fail!(
            "`places` names {}, which the range does not change; components that are not in it are listed only when it changes shared design-system sources",
            join(affected)
        );
    }
    let mut resolutions = BTreeMap::new();
    let mut new_items = Vec::new();
    for (path, answer) in &request.places {
        let resolution = match answer {
            PlaceAnswer::Item(item) => {
                open_item(lists, *item)?;
                Resolution::Item(*item)
            }
            PlaceAnswer::New(draft) => {
                let mut draft = draft.clone();
                if !draft.waits.contains(&Wait::Design(id)) {
                    draft.waits.push(Wait::Design(id));
                }
                let item = add_item(lists, &draft, Some(id), stamp)?;
                new_items.push(item);
                Resolution::Item(item)
            }
            PlaceAnswer::Matches(how) if how.trim().is_empty() => {
                fail!("place {path} is `matches` with no how it was checked")
            }
            PlaceAnswer::Matches(how) => Resolution::Matches(how.clone()),
        };
        resolutions.insert(path.clone(), resolution);
    }
    for (path, board) in &request.boards {
        if PlaceKind::of(path) != PlaceKind::Board {
            fail!("`boards` names {path}, which is no board file");
        }
        if board.link.trim().is_empty() {
            fail!("board {path} has an empty canvas link");
        }
    }
    if let Some(review) = &request.review {
        check_review(review)?;
    }
    let Some(design) = lists.design_mut(id) else { fail!("{id} is not an open design change") };
    design.worktree = worktree.to_string();
    // The change holds what the range changes: a stored place it no longer
    // touches (a board renamed before landing) leaves with its board and its
    // place on the review page.
    let dropped: BTreeSet<String> =
        design.places.keys().filter(|p| !changed.contains(*p) && !affected.contains(*p)).cloned().collect();
    design.places.retain(|path, _| !dropped.contains(path));
    design.boards.retain(|path, _| !dropped.contains(path));
    for section in &mut design.review {
        section.boards.retain(|b| !dropped.contains(&b.board));
    }
    design.places.extend(resolutions);
    design.boards.extend(request.boards.clone());
    if let Some(review) = &request.review {
        design.review = review.clone();
    }
    let unresolved: Vec<&str> =
        changed.iter().filter(|p| !design.places.contains_key(**p)).map(|p| p.as_str()).collect();
    if !unresolved.is_empty() {
        fail!(
            "design places left unresolved: {}; give each `item` (an open item), `new` (an item draft) or `matches` (how it was checked)",
            unresolved.join(", ")
        );
    }
    let unlinked: Vec<&str> = design
        .places
        .keys()
        .filter(|p| PlaceKind::of(p) == PlaceKind::Board && !design.boards.contains_key(*p))
        .map(String::as_str)
        .collect();
    if !unlinked.is_empty() {
        fail!(
            "boards without a canvas link: {}; give each under `boards` as {{\"link\": CANVAS_URL}}",
            unlinked.join(", ")
        );
    }
    if let Some(path) = design.boards.keys().find(|p| !design.places.contains_key(*p)) {
        fail!("`boards` names {path}, which is no place of this design change");
    }
    let on_page: BTreeSet<&str> = design.review.iter().flat_map(|s| &s.boards).map(|b| b.board.as_str()).collect();
    let missing: Vec<&str> = design
        .places
        .keys()
        .filter(|p| PlaceKind::of(p) == PlaceKind::Board && !on_page.contains(p.as_str()))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        fail!(
            "boards of the change missing from the review page: {}; the person reviews every board of a change on its page, so give each under `review`",
            missing.join(", ")
        );
    }
    if let Some(why) = &request.no_app_place {
        if why.trim().is_empty() {
            fail!("no_app_place is empty; say why shared design files changed and no drawing did");
        }
        let drawing = design.places.keys().any(|p| PlaceKind::of(p) == PlaceKind::Component);
        if (changes.sources.is_empty() && changes.shared.is_empty()) || !changes.places.is_empty() || drawing {
            fail!(
                "no_app_place is only for a range that changes shared design files and no board or component, and lists no component whose drawing changes"
            );
        }
        design.no_app_place = Some(why.clone());
    }
    needs_answer(design, changes, "the range")?;
    Ok(new_items)
}

/// A review page's own rules: every section has a heading, and every board on
/// it is a board file. (That a board is one of the repo's board places and
/// exists in the worktree is the command's, which reads git and the files.)
fn check_review(review: &[ReviewSection]) -> Result<()> {
    if review.is_empty() {
        fail!("`review` has no section; a review page has at least one heading");
    }
    for section in review {
        if section.heading.trim().is_empty() {
            fail!("a section of `review` has an empty heading");
        }
        for board in &section.boards {
            if PlaceKind::of(&board.board) != PlaceKind::Board {
                fail!("`review` names {}, which is no board file", board.board);
            }
            if board.caption.trim().is_empty() {
                fail!("the board {} on the review page has an empty caption", board.board);
            }
        }
    }
    Ok(())
}

/// Shared design files and sources change no place by themselves: the change
/// must list the components whose drawing changes (a source) or have a
/// resolved place, or say with `no_app_place` that no drawing changes.
fn needs_answer(design: &DesignChange, changes: &Changes, what: &str) -> Result<()> {
    if design.no_app_place.is_some() {
        return Ok(());
    }
    if !changes.sources.is_empty() {
        if design.places.keys().any(|p| PlaceKind::of(p) == PlaceKind::Component) {
            return Ok(());
        }
        fail!(
            "{what} changes shared design-system sources ({}); list under `places` each component whose drawing changes (an item or `matches`), or say with `no_app_place` that no drawing changes",
            join(&changes.sources)
        );
    }
    if !changes.shared.is_empty() && changes.places.is_empty() && changes.spec.is_empty() {
        fail!(
            "{what} changes only shared design files ({}); say why no board or component changed with `no_app_place`",
            join(&changes.shared)
        );
    }
    Ok(())
}

fn join(set: &BTreeSet<String>) -> String {
    set.iter().map(String::as_str).collect::<Vec<_>>().join(", ")
}

/// Records a new design change of <repo> for <changes>.
pub fn record(
    lists: &mut Lists,
    repo: &str,
    worktree: &str,
    request: &DesignRequest,
    changes: &Changes,
    affected: &BTreeSet<String>,
    stamp: &Stamp,
) -> Result<Recorded> {
    let title = require_text("title", request.title.as_ref())?;
    if let Some(fault) = crate::lists::check::title_fault(&title) {
        fail!("{fault}");
    }
    let details = require_text("details", request.details.as_ref())?;
    if request.review.is_none() {
        fail!("a design change needs its `review`: the headings, words and boards of the page the person reviews it on");
    }
    let design = lists.alloc_id();
    lists.designs.push(DesignChange {
        id: design,
        title,
        details,
        repo: repo.to_string(),
        worktree: worktree.to_string(),
        places: BTreeMap::new(),
        boards: BTreeMap::new(),
        review: Vec::new(),
        no_app_place: None,
        approved: None,
        opened: stamp.clone(),
        landed: Vec::new(),
    });
    let new_items = resolve(lists, design, request, worktree, changes, affected, stamp)?;
    Ok(Recorded { design, new_items })
}

/// The open design change <id>.
fn open_design(lists: &Lists, id: Id) -> Result<&DesignChange> {
    Ok(match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located { kind, .. }) if kind != Kind::Design => fail!("{id} is a {}, not a design change", kind.name()),
        Some(Located { standing: Standing::Closed(_), .. }) => fail!("design change {id} has already landed"),
        Some(_) => lists.design(id).expect("located open"),
    })
}

/// The open design change <id>, of <repo>.
fn open_design_of<'a>(lists: &'a Lists, id: Id, repo: &str) -> Result<&'a DesignChange> {
    let design = open_design(lists, id)?;
    if design.repo != repo {
        fail!("design change {id} is of repo {}, not {repo}", design.repo);
    }
    Ok(design)
}

/// Holds open design change <id> to the places <changes> holds: the answers
/// of <request> are added, and a stored place the range no longer changes
/// leaves with its board and its place on the review page. What the person approved is no
/// longer what is drawn, so the approval is cleared.
pub fn amend(
    lists: &mut Lists,
    id: Id,
    repo: &str,
    worktree: &str,
    request: &DesignRequest,
    changes: &Changes,
    affected: &BTreeSet<String>,
    stamp: &Stamp,
) -> Result<Recorded> {
    open_design_of(lists, id, repo)?;
    let design = lists.design_mut(id).expect("open");
    design.approved = None;
    if let Some(title) = &request.title {
        if title.trim().is_empty() {
            fail!("title is empty");
        }
        if let Some(fault) = crate::lists::check::title_fault(title) {
            fail!("{fault}");
        }
        design.title = title.clone();
    }
    if let Some(details) = &request.details {
        design.details = require_text("details", Some(details))?;
    }
    let new_items = resolve(lists, id, request, worktree, changes, affected, stamp)?;
    Ok(Recorded { design: id, new_items })
}

/// Is every place <changes> holds resolved in open design change <id>, and
/// has the person approved it? Compares by path, so a rebased commit checks
/// the same. Names each place left over.
pub fn check_changes(lists: &Lists, id: Id, repo: &str, changes: &Changes) -> Result<()> {
    let n = id.0;
    let design = open_design_of(lists, id, repo)?;
    if !changes.is_design_change() {
        fail!("the commits change no design files, so design change {id} has nothing to check");
    }
    let unresolved: Vec<&str> =
        changes.resolvable().into_iter().filter(|p| !design.places.contains_key(*p)).map(|p| p.as_str()).collect();
    if !unresolved.is_empty() {
        fail!(
            "design change {id} does not resolve {}; `builds design amend {n}` adds them",
            unresolved.join(", ")
        );
    }
    needs_answer(design, changes, "the commits").map_err(|error| match error {
        crate::fail::Fail::Message(message) => {
            crate::fail::Fail::Message(format!("{message} (design change {id}; `builds design amend {}` adds it)", id.0))
        }
        other => other,
    })?;
    if design.approved.is_none() {
        fail!(
            "design change {id} waits on the person's review; once they have said to land it, `builds design approve {n} --by-person` records that"
        );
    }
    Ok(())
}

/// Records the person's approval of open design change <id>. The same
/// approval again is recorded once.
pub fn approve(lists: &mut Lists, id: Id, stamp: &Stamp) -> Result<()> {
    open_design(lists, id)?;
    let design = lists.design_mut(id).expect("open");
    if design.approved.is_none() {
        design.approved = Some(stamp.clone());
    }
    Ok(())
}

/// What `landed` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Landed {
    /// The change closed; these items no longer wait on it and are ready.
    Closed { ready: Vec<(Id, String)> },
    /// The same commits were recorded before.
    Already,
}

/// Did design change <id> already land as exactly <commits>? A change that
/// landed as other commits is refused; one that is open is not landed yet.
pub fn already_landed(lists: &Lists, id: Id, repo: &str, commits: &[String]) -> Result<bool> {
    let Some(closed) = lists.closed_record(id) else { return Ok(false) };
    let (Record::Design(design), How::Landed(was)) = (&closed.record, &closed.how) else {
        fail!("{id} is not a design change that landed");
    };
    if design.repo != repo {
        fail!("design change {id} is of repo {}, not {repo}", design.repo);
    }
    let (mut was, mut now) = (was.clone(), commits.to_vec());
    was.sort();
    now.sort();
    if was != now {
        fail!("design change {id} already landed as {}, not as {}", was.join(" "), now.join(" "));
    }
    Ok(true)
}

/// Records that design change <id> landed as <commits> (full hashes) and
/// closes it. The same commits again are recorded once; other commits for a
/// change that landed are refused.
pub fn landed(lists: &mut Lists, id: Id, repo: &str, commits: &[String], stamp: &Stamp) -> Result<Landed> {
    if commits.is_empty() {
        fail!("design landed names no commits");
    }
    if already_landed(lists, id, repo, commits)? {
        return Ok(Landed::Already);
    }
    open_design_of(lists, id, repo)?;
    lists
        .design_mut(id)
        .expect("open")
        .landed
        .push(DesignLanding { commits: commits.to_vec(), at: stamp.at });
    crate::lists::record::close(lists, id, How::Landed(commits.to_vec()), stamp)?;
    let ready = lists
        .items
        .iter()
        .filter(|item| item.waits.contains(&Wait::Design(id)) && derive::item_state(lists, item) == ItemState::Ready)
        .map(|item| (item.id, item.title.clone()))
        .collect();
    Ok(Landed::Closed { ready })
}

fn resolution_words(lists: &Lists, resolution: &Resolution) -> String {
    match resolution {
        Resolution::Item(id) => match lists.locate(*id) {
            Some(Located { standing: Standing::Open, title, .. }) => {
                let state = lists.item(*id).map(|i| derive::item_state(lists, i).label()).unwrap_or("?");
                format!("item {id} [{state}] {title}")
            }
            Some(Located { title, .. }) => format!("item {id} [closed] {title}"),
            None => format!("item {id}"),
        },
        Resolution::Matches(how) => format!("matches: {how}"),
    }
}

fn describe(lists: &Lists, design: &DesignChange, landed: Option<&crate::lists::Closed>) -> String {
    let mut out = String::new();
    let state = match landed {
        Some(closed) => format!("landed {}", local_minute(closed.closed.at)),
        None => "open".to_string(),
    };
    let _ = writeln!(out, "{} {} [{state}] repo {}", design.id, design.title, design.repo);
    let _ = writeln!(out, "opened {} by {}", local_minute(design.opened.at), design.opened.by);
    match &design.approved {
        Some(stamp) => {
            let _ = writeln!(out, "approved {} (recorded by {})", local_minute(stamp.at), stamp.by);
        }
        None if landed.is_none() => {
            let _ = writeln!(out, "waiting on the person's review");
        }
        None => {}
    }
    let _ = writeln!(out, "\n{}\n", design.details.trim_end());
    for (path, resolution) in &design.places {
        let _ = writeln!(out, "  {path}  {}", resolution_words(lists, resolution));
    }
    for (path, board) in &design.boards {
        let _ = writeln!(out, "  board {path}  {}", board.link);
    }
    let _ = writeln!(out, "  worktree {}", design.worktree);
    for section in &design.review {
        let _ = writeln!(out, "  review: {}", section.heading);
        for board in &section.boards {
            let _ = writeln!(out, "    {} ({}) {}", board.board, board.theme.word(), board.caption);
        }
    }
    if let Some(why) = &design.no_app_place {
        let _ = writeln!(out, "  no drawing changes: {why}");
    }
    for landing in &design.landed {
        let _ = writeln!(out, "  landed {} as {}", local_minute(landing.at), landing.commits.join(" "));
    }
    out
}

/// `design show`: an open or landed design change.
pub fn show(lists: &Lists, id: Id) -> Result<String> {
    match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located { kind, .. }) if kind != Kind::Design => fail!("{id} is a {}, not a design change", kind.name()),
        Some(Located { standing: Standing::Open, .. }) => Ok(describe(lists, lists.design(id).expect("open"), None)),
        Some(Located { standing: Standing::Closed(closed), .. }) => {
            let Record::Design(design) = &closed.record else { unreachable!("located as a design change") };
            Ok(describe(lists, design, Some(closed)))
        }
    }
}

/// `design list`: the open design changes, one line each.
pub fn list(lists: &Lists) -> String {
    let mut out = String::new();
    for design in &lists.designs {
        let waiting = lists.items.iter().filter(|i| i.waits.contains(&Wait::Design(design.id))).count();
        let _ = writeln!(
            out,
            "{} {} ({}, repo {}, {} place{}, {waiting} item{} waiting)",
            design.id,
            design.title,
            if design.approved.is_some() { "approved" } else { "waiting on review" },
            design.repo,
            design.places.len(),
            if design.places.len() == 1 { "" } else { "s" },
            if waiting == 1 { "" } else { "s" },
        );
    }
    out
}

#[cfg(test)]
#[path = "design_tests.rs"]
mod tests;

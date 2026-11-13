//! Design changes: a product design change (a board or a design-system
//! component) recorded before it lands, with every design place it changes
//! resolved to the app work that follows or to how it was checked that none
//! does. These are the operations on the state a write holds under the lock;
//! `design_command` is the commands around them, `design_paths` reads git.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use serde::Deserialize;

use crate::fail;
use crate::fail::Result;
use crate::lists::derive::{self, ItemState};
use crate::lists::design_paths::Changes;
use crate::lists::draft::{ItemDraft, add_item};
use crate::lists::{DesignChange, DesignLanding, How, Id, Kind, Lists, Located, Record, Resolution, Stamp, Standing, Wait};
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
    /// For every design place the range changes.
    #[serde(default)]
    pub places: BTreeMap<String, PlaceAnswer>,
    /// Why shared design files changed with no board or component.
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
/// be resolved and, when only shared files changed, `no_app_place` must be
/// said. Refuses with every unresolved place named.
fn resolve(lists: &mut Lists, id: Id, request: &DesignRequest, changes: &Changes, stamp: &Stamp) -> Result<Vec<Id>> {
    if changes.is_empty() {
        fail!("the range {} changes no design files", request.range);
    }
    let stray: Vec<&str> =
        request.places.keys().filter(|path| !changes.places.contains(*path)).map(String::as_str).collect();
    if !stray.is_empty() {
        fail!("`places` names {}, which the range does not change", stray.join(", "));
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
    let Some(design) = lists.design_mut(id) else { fail!("{id} is not an open design change") };
    design.places.extend(resolutions);
    let unresolved: Vec<&str> =
        changes.places.iter().filter(|p| !design.places.contains_key(*p)).map(String::as_str).collect();
    if !unresolved.is_empty() {
        fail!(
            "design places left unresolved: {}; give each `item` (an open item), `new` (an item draft) or `matches` (how it was checked)",
            unresolved.join(", ")
        );
    }
    if let Some(why) = &request.no_app_place {
        if why.trim().is_empty() {
            fail!("no_app_place is empty; say why shared design files changed with no board or component");
        }
        if !changes.places.is_empty() || changes.shared.is_empty() {
            fail!("no_app_place is only for a range that changes shared design files and no board or component");
        }
        design.no_app_place = Some(why.clone());
    }
    if changes.places.is_empty() && design.no_app_place.is_none() {
        fail!(
            "the range changes only shared design files ({}); say why no board or component changed with `no_app_place`",
            join(&changes.shared)
        );
    }
    Ok(new_items)
}

fn join(set: &BTreeSet<String>) -> String {
    set.iter().map(String::as_str).collect::<Vec<_>>().join(", ")
}

/// Records a new design change of <repo> for <changes>.
pub fn record(lists: &mut Lists, repo: &str, request: &DesignRequest, changes: &Changes, stamp: &Stamp) -> Result<Recorded> {
    let title = require_text("title", request.title.as_ref())?;
    if let Some(fault) = crate::lists::check::title_fault(&title) {
        fail!("{fault}");
    }
    let details = require_text("details", request.details.as_ref())?;
    let design = lists.alloc_id();
    lists.designs.push(DesignChange {
        id: design,
        title,
        details,
        repo: repo.to_string(),
        places: BTreeMap::new(),
        no_app_place: None,
        opened: stamp.clone(),
        landed: Vec::new(),
    });
    let new_items = resolve(lists, design, request, changes, stamp)?;
    Ok(Recorded { design, new_items })
}

/// The open design change <id>, of <repo>.
fn open_design<'a>(lists: &'a Lists, id: Id, repo: &str) -> Result<&'a DesignChange> {
    let design = match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located { kind, .. }) if kind != Kind::Design => fail!("{id} is a {}, not a design change", kind.name()),
        Some(Located { standing: Standing::Closed(_), .. }) => fail!("design change {id} has already landed"),
        Some(_) => lists.design(id).expect("located open"),
    };
    if design.repo != repo {
        fail!("design change {id} is of repo {}, not {repo}", design.repo);
    }
    Ok(design)
}

/// Adds the places <changes> holds to open design change <id>: the answers
/// of <request> merge over the places it has.
pub fn amend(lists: &mut Lists, id: Id, repo: &str, request: &DesignRequest, changes: &Changes, stamp: &Stamp) -> Result<Recorded> {
    open_design(lists, id, repo)?;
    let design = lists.design_mut(id).expect("open");
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
    let new_items = resolve(lists, id, request, changes, stamp)?;
    Ok(Recorded { design: id, new_items })
}

/// Is every place <changes> holds resolved in open design change <id>?
/// Compares by path, so a rebased commit checks the same. Names each
/// place left over.
pub fn check_changes(lists: &Lists, id: Id, repo: &str, changes: &Changes) -> Result<()> {
    let design = open_design(lists, id, repo)?;
    if changes.is_empty() {
        fail!("the commits change no design files, so design change {id} has nothing to check");
    }
    let unresolved: Vec<&str> =
        changes.places.iter().filter(|p| !design.places.contains_key(*p)).map(String::as_str).collect();
    if !unresolved.is_empty() {
        fail!(
            "design change {id} does not resolve {}; `builds design amend {id}` adds them",
            unresolved.join(", ")
        );
    }
    if changes.places.is_empty() && design.no_app_place.is_none() {
        fail!(
            "the commits change only shared design files ({}) and design change {id} says why none is a board or component (`no_app_place`); `builds design amend {id}` adds it",
            join(&changes.shared)
        );
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
    open_design(lists, id, repo)?;
    lists
        .design_mut(id)
        .expect("open")
        .landed
        .push(DesignLanding { commits: commits.to_vec(), at: stamp.at });
    crate::lists::draft::close(lists, id, How::Landed(commits.to_vec()), stamp)?;
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
    let _ = writeln!(out, "\n{}\n", design.details.trim_end());
    for (path, resolution) in &design.places {
        let _ = writeln!(out, "  {path}  {}", resolution_words(lists, resolution));
    }
    if let Some(why) = &design.no_app_place {
        let _ = writeln!(out, "  no board or component: {why}");
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
            "{} {} (repo {}, {} place{}, {waiting} item{} waiting)",
            design.id,
            design.title,
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
